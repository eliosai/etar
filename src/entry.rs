//! Filesystem entry vocabulary shared by pack and open

use crate::error::Error;
use std::path::{Component, Path, PathBuf};

/// Unix metadata describing one archive entry
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Meta {
    /// Permission and type bits
    pub mode: u32,
    /// Owning user id
    pub uid: u32,
    /// Owning group id
    pub gid: u32,
    /// Modification time in seconds since the unix epoch
    pub mtime_unix_secs: u64,
    /// What kind of entry this is
    pub kind: Kind,
}

/// The kind of an archive entry
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
    /// A regular file carrying a body
    File,
    /// A directory
    Dir,
    /// A symbolic link
    Symlink {
        /// Untrusted link target as stored; etar never follows or materializes it
        target: PathBuf,
    },
    /// A hard link to another entry in the same archive
    Hardlink {
        /// Untrusted in-archive target; etar never resolves or materializes it
        target: PathBuf,
    },
}

impl Meta {
    /// Build entry metadata from its unix fields
    pub fn new(mode: u32, uid: u32, gid: u32, mtime_unix_secs: u64, kind: Kind) -> Self {
        Self {
            mode,
            uid,
            gid,
            mtime_unix_secs,
            kind,
        }
    }
}

/// A relative, in-tree, NUL-free entry path
///
/// # Example
///
/// ```
/// use etar::{EntryPath, Error};
///
/// let path = EntryPath::new("etc/hosts").unwrap();
/// assert_eq!(path.as_bytes(), b"etc/hosts");
/// assert!(matches!(EntryPath::new("../escape"), Err(Error::ParentEscape)));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SafePath(PathBuf);

impl SafePath {
    /// Validate a path as relative, in-tree, and NUL-free under Linux path semantics
    pub fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        validate(path)?;
        Ok(Self(path.to_path_buf()))
    }

    /// The validated path
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// The original path bytes on this platform
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_os_str().as_encoded_bytes()
    }
}

/// Reject empty, NUL-bearing, absolute, or ascending paths
fn validate(path: &Path) -> Result<(), Error> {
    let raw = path.as_os_str();
    if raw.is_empty() {
        return Err(Error::EmptyPath);
    }
    if raw.as_encoded_bytes().contains(&0) {
        return Err(Error::NulPath);
    }
    for component in path.components() {
        reject_unsafe(component)?;
    }
    Ok(())
}

/// Reject root, drive prefix, and parent components
fn reject_unsafe(component: Component<'_>) -> Result<(), Error> {
    match component {
        Component::Prefix(_) | Component::RootDir => Err(Error::AbsolutePath),
        Component::ParentDir => Err(Error::ParentEscape),
        Component::CurDir | Component::Normal(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    #[test]
    fn test_safepath_accepts_relative() {
        let actual = SafePath::new("bin/sh").unwrap();
        assert_eq!(actual.as_path(), Path::new("bin/sh"));
    }

    #[test]
    fn test_safepath_accepts_current_dir_prefix() {
        let actual = SafePath::new("./bin/sh").unwrap();
        assert_eq!(actual.as_path(), Path::new("./bin/sh"));
    }

    #[test]
    fn test_safepath_rejects_absolute() {
        assert!(matches!(
            SafePath::new("/etc/passwd"),
            Err(Error::AbsolutePath)
        ));
    }

    #[test]
    fn test_safepath_rejects_leading_parent() {
        assert!(matches!(
            SafePath::new("../../etc/passwd"),
            Err(Error::ParentEscape)
        ));
    }

    #[test]
    fn test_safepath_rejects_embedded_parent() {
        assert!(matches!(
            SafePath::new("a/../../b"),
            Err(Error::ParentEscape)
        ));
    }

    #[test]
    fn test_safepath_rejects_empty() {
        assert!(matches!(SafePath::new(""), Err(Error::EmptyPath)));
    }

    #[test]
    fn test_safepath_rejects_nul() {
        assert!(matches!(SafePath::new("a\0b"), Err(Error::NulPath)));
    }

    /// One path component biased toward the dangerous alphabet
    fn component() -> impl Strategy<Value = String> {
        prop_oneof![Just("..".to_owned()), Just(".".to_owned()), "[a-z]{1,5}"]
    }

    proptest! {
        #[test]
        fn prop_rejected_iff_escape_present(parts in proptest::collection::vec(component(), 1..8)) {
            let raw = parts.join("/");
            let result = SafePath::new(&raw);
            if parts.iter().any(|p| p == "..") {
                prop_assert!(matches!(result, Err(Error::ParentEscape)));
            } else {
                let safe = result.expect("non-escape path is accepted");
                for c in safe.as_path().components() {
                    prop_assert!(matches!(c, Component::Normal(_) | Component::CurDir));
                }
            }
        }

        #[test]
        fn prop_leading_slash_is_absolute(parts in proptest::collection::vec("[a-z]{1,5}", 1..6)) {
            let raw = format!("/{}", parts.join("/"));
            prop_assert!(matches!(SafePath::new(&raw), Err(Error::AbsolutePath)));
        }
    }
}
