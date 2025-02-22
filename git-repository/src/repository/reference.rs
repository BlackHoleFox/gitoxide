use std::convert::TryInto;

use git_actor as actor;
use git_hash::ObjectId;
use git_lock as lock;
use git_ref::{
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
    FullName, PartialNameRef, Target,
};

use crate::{bstr::BString, ext::ReferenceExt, reference, Reference};

const DEFAULT_LOCK_MODE: git_lock::acquire::Fail = git_lock::acquire::Fail::Immediately;

/// Obtain and alter references comfortably
impl crate::Repository {
    /// Create a lightweight tag with given `name` (and without `refs/tags/` prefix) pointing to the given `target`, and return it as reference.
    ///
    /// It will be created with `constraint` which is most commonly to [only create it][PreviousValue::MustNotExist]
    /// or to [force overwriting a possibly existing tag](PreviousValue::Any).
    pub fn tag_reference(
        &self,
        name: impl AsRef<str>,
        target: impl Into<ObjectId>,
        constraint: PreviousValue,
    ) -> Result<Reference<'_>, reference::edit::Error> {
        let id = target.into();
        let mut edits = self.edit_reference(
            RefEdit {
                change: Change::Update {
                    log: Default::default(),
                    expected: constraint,
                    new: Target::Peeled(id),
                },
                name: format!("refs/tags/{}", name.as_ref()).try_into()?,
                deref: false,
            },
            DEFAULT_LOCK_MODE,
            None,
        )?;
        assert_eq!(edits.len(), 1, "reference splits should ever happen");
        let edit = edits.pop().expect("exactly one item");
        Ok(crate::Reference {
            inner: git_ref::Reference {
                name: edit.name,
                target: id.into(),
                peeled: None,
            },
            repo: self,
        })
    }

    /// Returns the currently set namespace for references, or `None` if it is not set.
    ///
    /// Namespaces allow to partition references, and is configured per `Easy`.
    pub fn namespace(&self) -> Option<&git_ref::Namespace> {
        self.refs.namespace.as_ref()
    }

    /// Remove the currently set reference namespace and return it, affecting only this `Easy`.
    pub fn clear_namespace(&mut self) -> Option<git_ref::Namespace> {
        self.refs.namespace.take()
    }

    /// Set the reference namespace to the given value, like `"foo"` or `"foo/bar"`.
    ///
    /// Note that this value is shared across all `Easy…` instances as the value is stored in the shared `Repository`.
    pub fn set_namespace<'a, Name, E>(
        &mut self,
        namespace: Name,
    ) -> Result<Option<git_ref::Namespace>, git_validate::refname::Error>
    where
        Name: TryInto<&'a PartialNameRef, Error = E>,
        git_validate::refname::Error: From<E>,
    {
        let namespace = git_ref::namespace::expand(namespace)?;
        Ok(self.refs.namespace.replace(namespace))
    }

    // TODO: more tests or usage
    /// Create a new reference with `name`, like `refs/heads/branch`, pointing to `target`, adhering to `constraint`
    /// during creation and writing `log_message` into the reflog. Note that a ref-log will be written even if `log_message` is empty.
    ///
    /// The newly created Reference is returned.
    pub fn reference<Name, E>(
        &self,
        name: Name,
        target: impl Into<ObjectId>,
        constraint: PreviousValue,
        log_message: impl Into<BString>,
    ) -> Result<Reference<'_>, reference::edit::Error>
    where
        Name: TryInto<FullName, Error = E>,
        reference::edit::Error: From<E>,
    {
        let name = name.try_into()?;
        let id = target.into();
        let mut edits = self.edit_reference(
            RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: log_message.into(),
                    },
                    expected: constraint,
                    new: Target::Peeled(id),
                },
                name,
                deref: false,
            },
            DEFAULT_LOCK_MODE,
            None,
        )?;
        assert_eq!(
            edits.len(),
            1,
            "only one reference can be created, splits aren't possible"
        );

        Ok(git_ref::Reference {
            name: edits.pop().expect("exactly one edit").name,
            target: Target::Peeled(id),
            peeled: None,
        }
        .attach(self))
    }

    /// Edit a single reference as described in `edit`, handle locks via `lock_mode` and write reference logs as `log_committer`.
    ///
    /// One or more `RefEdit`s  are returned - symbolic reference splits can cause more edits to be performed. All edits have the previous
    /// reference values set to the ones encountered at rest after acquiring the respective reference's lock.
    pub fn edit_reference(
        &self,
        edit: RefEdit,
        lock_mode: lock::acquire::Fail,
        log_committer: Option<&actor::Signature>,
    ) -> Result<Vec<RefEdit>, reference::edit::Error> {
        self.edit_references(Some(edit), lock_mode, log_committer)
    }

    /// Edit one or more references as described by their `edits`, with `lock_mode` deciding on how to handle competing
    /// transactions. `log_committer` is the name appearing in reference logs.
    ///
    /// Returns all reference edits, which might be more than where provided due the splitting of symbolic references, and
    /// whose previous (_old_) values are the ones seen on in storage after the reference was locked.
    pub fn edit_references(
        &self,
        edits: impl IntoIterator<Item = RefEdit>,
        lock_mode: lock::acquire::Fail,
        log_committer: Option<&actor::Signature>,
    ) -> Result<Vec<RefEdit>, reference::edit::Error> {
        let committer_storage;
        let committer = match log_committer {
            Some(c) => c,
            None => {
                committer_storage = self.committer();
                &committer_storage
            }
        };
        self.refs
            .transaction()
            .prepare(edits, lock_mode)?
            .commit(committer.to_ref())
            .map_err(Into::into)
    }

    /// Return the repository head, an abstraction to help dealing with the `HEAD` reference.
    ///
    /// The `HEAD` reference can be in various states, for more information, the documentation of [`Head`][crate::Head].
    pub fn head(&self) -> Result<crate::Head<'_>, reference::find::existing::Error> {
        let head = self.find_reference("HEAD")?;
        Ok(match head.inner.target {
            Target::Symbolic(branch) => match self.find_reference(&branch) {
                Ok(r) => crate::head::Kind::Symbolic(r.detach()),
                Err(reference::find::existing::Error::NotFound) => crate::head::Kind::Unborn(branch),
                Err(err) => return Err(err),
            },
            Target::Peeled(target) => crate::head::Kind::Detached {
                target,
                peeled: head.inner.peeled,
            },
        }
        .attach(self))
    }

    /// Resolve the `HEAD` reference, follow and peel its target and obtain its object id.
    ///
    /// Note that this may fail for various reasons, most notably because the repository
    /// is freshly initialized and doesn't have any commits yet.
    ///
    /// Also note that the returned id is likely to point to a commit, but could also
    /// point to a tree or blob. It won't, however, point to a tag as these are always peeled.
    pub fn head_id(&self) -> Result<crate::Id<'_>, crate::reference::head_id::Error> {
        let mut head = self.head()?;
        head.peel_to_id_in_place()
            .ok_or_else(|| crate::reference::head_id::Error::Unborn {
                name: head.referent_name().expect("unborn").to_owned(),
            })?
            .map_err(Into::into)
    }

    /// Return the name to the symbolic reference `HEAD` points to, or `None` if the head is detached.
    pub fn head_name(&self) -> Result<Option<git_ref::FullName>, crate::reference::find::existing::Error> {
        Ok(self.head()?.referent_name().map(|n| n.to_owned()))
    }

    /// Return the commit object the `HEAD` reference currently points to after peeling it fully.
    ///
    /// Note that this may fail for various reasons, most notably because the repository
    /// is freshly initialized and doesn't have any commits yet. It could also fail if the
    /// head does not point to a commit.
    pub fn head_commit(&self) -> Result<crate::Commit<'_>, crate::reference::head_commit::Error> {
        Ok(self.head()?.peel_to_commit_in_place()?)
    }

    /// Find the reference with the given partial or full `name`, like `main`, `HEAD`, `heads/branch` or `origin/other`,
    /// or return an error if it wasn't found.
    ///
    /// Consider [`try_find_reference(…)`][crate::Repository::try_find_reference()] if the reference might not exist
    /// without that being considered an error.
    pub fn find_reference<'a, Name, E>(&self, name: Name) -> Result<Reference<'_>, reference::find::existing::Error>
    where
        Name: TryInto<&'a PartialNameRef, Error = E>,
        git_ref::file::find::Error: From<E>,
    {
        self.try_find_reference(name)?
            .ok_or(reference::find::existing::Error::NotFound)
    }

    /// Return a platform for iterating references.
    ///
    /// Common kinds of iteration are [all][crate::reference::iter::Platform::all()] or [prefixed][crate::reference::iter::Platform::prefixed()]
    /// references.
    pub fn references(&self) -> Result<crate::reference::iter::Platform<'_>, crate::reference::iter::Error> {
        Ok(crate::reference::iter::Platform {
            platform: self.refs.iter()?,
            repo: self,
        })
    }

    /// Try to find the reference named `name`, like `main`, `heads/branch`, `HEAD` or `origin/other`, and return it.
    ///
    /// Otherwise return `None` if the reference wasn't found.
    /// If the reference is expected to exist, use [`find_reference()`][crate::Repository::find_reference()].
    pub fn try_find_reference<'a, Name, E>(&self, name: Name) -> Result<Option<Reference<'_>>, reference::find::Error>
    where
        Name: TryInto<&'a PartialNameRef, Error = E>,
        git_ref::file::find::Error: From<E>,
    {
        let state = self;
        match state.refs.try_find(name) {
            Ok(r) => match r {
                Some(r) => Ok(Some(Reference::from_ref(r, self))),
                None => Ok(None),
            },
            Err(err) => Err(err.into()),
        }
    }
}
