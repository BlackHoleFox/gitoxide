pub trait Sealed {}

impl Sealed for git_ref::Reference {}

/// Extensions for [references][git_ref::Reference].
pub trait ReferenceExt {
    /// Attach [`Repository`][crate::Repository] to the given reference. It can be detached later with [`detach()]`.
    fn attach(self, repo: &crate::Repository) -> crate::Reference<'_>;
}

impl ReferenceExt for git_ref::Reference {
    fn attach(self, repo: &crate::Repository) -> crate::Reference<'_> {
        crate::Reference::from_ref(self, repo)
    }
}
