use std::{cmp::Ordering, convert::TryFrom};

use quick_error::quick_error;

use crate::{oid, ObjectId, Prefix};

const MIN_HEX_LEN: usize = 4;

quick_error! {
    /// The error returned by [Prefix::try_from_id()][super::Prefix::try_from_id()].
    #[derive(Debug)]
    #[allow(missing_docs)]
    pub enum Error {
        TooShort { hex_len: usize } {
            display("The minimum hex length of a short object id is {}, got {}", MIN_HEX_LEN, hex_len)
        }
        TooLong { object_kind: crate::Kind, hex_len: usize } {
            display("An object of kind {} cannot be larger than {} in hex, but {} was requested", object_kind, object_kind.len_in_hex(), hex_len)
        }
    }
}

///
pub mod from_hex {
    use quick_error::quick_error;
    quick_error! {
        /// The error returned by [Prefix::from_hex][super::Prefix::from_hex()].
        #[derive(Debug, PartialEq)]
        #[allow(missing_docs)]
        pub enum Error {
            TooShort { hex_len: usize } {
                display("The minimum hex length of a short object id is {}, got {}", super::MIN_HEX_LEN, hex_len)
            }
            TooLong { hex_len: usize } {
                display("An id cannot be larger than {} chars in hex, but {} was requested", crate::Kind::longest().len_in_hex(), hex_len)
            }
            Invalid { c: char, index: usize } {
                display("Invalid character {} at position {}", c, index)
            }
        }
    }
}

impl Prefix {
    /// Create a new instance by taking a full `id` as input and truncating it to `hex_len`.
    ///
    /// For instance, with `hex_len` of 7 the resulting prefix is 3.5 bytes, or 3 bytes and 4 bits
    /// wide, with all other bytes and bits set to zero.
    pub fn new(id: impl AsRef<oid>, hex_len: usize) -> Result<Self, Error> {
        let id = id.as_ref();
        if hex_len > id.kind().len_in_hex() {
            Err(Error::TooLong {
                object_kind: id.kind(),
                hex_len,
            })
        } else if hex_len < MIN_HEX_LEN {
            Err(Error::TooShort { hex_len })
        } else {
            let mut prefix = ObjectId::null(id.kind());
            let b = prefix.as_mut_slice();
            let copy_len = (hex_len + 1) / 2;
            b[..copy_len].copy_from_slice(&id.as_bytes()[..copy_len]);
            if hex_len % 2 == 1 {
                b[hex_len / 2] &= 0xf0;
            }

            Ok(Prefix { bytes: prefix, hex_len })
        }
    }

    /// Returns the prefix as object id.
    ///
    /// Note that it may be deceptive to use given that it looks like a full
    /// object id, even though its post-prefix bytes/bits are set to zero.
    pub fn as_oid(&self) -> &oid {
        &self.bytes
    }

    /// Return the amount of hexadecimal characters that are set in the prefix.
    ///
    /// This gives the prefix a granularity of 4 bits.
    pub fn hex_len(&self) -> usize {
        self.hex_len
    }

    /// Provided with candidate id which is a full hash, determine how this prefix compares to it,
    /// only looking at the prefix bytes, ignoring everything behind that.
    pub fn cmp_oid(&self, candidate: &oid) -> Ordering {
        let common_len = self.hex_len / 2;

        self.bytes.as_bytes()[..common_len]
            .cmp(&candidate.as_bytes()[..common_len])
            .then(if self.hex_len % 2 == 1 {
                let half_byte_idx = self.hex_len / 2;
                self.bytes.as_bytes()[half_byte_idx].cmp(&(candidate.as_bytes()[half_byte_idx] & 0xf0))
            } else {
                Ordering::Equal
            })
    }

    /// Create an instance from the given hexadecimal prefix `value`, e.g. `35e77c16` would yield a `Prefix` with `hex_len()` = 8.
    pub fn from_hex(value: &str) -> Result<Self, from_hex::Error> {
        use hex::FromHex;
        let hex_len = value.len();

        if hex_len > crate::Kind::longest().len_in_hex() {
            return Err(from_hex::Error::TooLong { hex_len });
        } else if hex_len < MIN_HEX_LEN {
            return Err(from_hex::Error::TooShort { hex_len });
        };

        let src = if value.len() % 2 == 0 {
            Vec::from_hex(value)
        } else {
            let mut buf = [0u8; crate::Kind::longest().len_in_hex()];
            buf[..value.len()].copy_from_slice(value.as_bytes());
            buf[value.len()] = b'0';
            Vec::from_hex(&buf[..value.len() + 1])
        }
        .map_err(|e| match e {
            hex::FromHexError::InvalidHexCharacter { c, index } => from_hex::Error::Invalid { c, index },
            hex::FromHexError::OddLength | hex::FromHexError::InvalidStringLength => panic!("This is already checked"),
        })?;

        let mut bytes = ObjectId::null(crate::Kind::from_hex_len(value.len()).expect("hex-len is already checked"));
        let dst = bytes.as_mut_slice();
        let copy_len = src.len();
        dst[..copy_len].copy_from_slice(&src);

        Ok(Prefix { bytes, hex_len })
    }
}

/// Create an instance from the given hexadecimal prefix, e.g. `35e77c16` would yield a `Prefix`
/// with `hex_len()` = 8.
impl TryFrom<&str> for Prefix {
    type Error = from_hex::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Prefix::from_hex(value)
    }
}

impl std::fmt::Display for Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.bytes.to_hex_with_len(self.hex_len).fmt(f)
    }
}
