use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
};

use crate::ApplicationStageError;

pub(super) fn validate(path: &Path) -> Result<(), ApplicationStageError> {
    validate_below(Path::new("/"), path, 0)
}

// Production always starts at the filesystem root with UID 0. A separate
// anchor lets unprivileged tests exercise the same walk in isolated fixtures.
// Check parents before descending: an untrusted directory must never become
// the authority for inspecting its children. Other privileged writers remain
// outside this protection; the store's exclusive lock serializes this process.
fn validate_below(anchor: &Path, path: &Path, owner: u32) -> Result<(), ApplicationStageError> {
    let relative = path
        .strip_prefix(anchor)
        .map_err(|_| ApplicationStageError::UnsafeRoot)?;
    let check = |directory: &Path| {
        let metadata =
            fs::symlink_metadata(directory).map_err(|_| ApplicationStageError::UnsafeRoot)?;
        if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o022 != 0 {
            return Err(ApplicationStageError::UnsafeRoot);
        }
        Ok(())
    };
    check(anchor)?;
    let mut current = anchor.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(ApplicationStageError::UnsafeRoot);
        };
        current.push(name);
        check(&current)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn protected_walk_rejects_mutable_ancestors_and_rechecks_changes() {
        let temporary = tempfile::tempdir().unwrap();
        let anchor = temporary.path();
        let owner = fs::metadata(anchor).unwrap().uid();
        let parent = anchor.join("parent");
        let store = parent.join("store");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&store).unwrap();
        for directory in [anchor, &parent, &store] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        validate_below(anchor, &store, owner).unwrap();
        for directory in [anchor, &parent, &store] {
            for mode in [0o720, 0o702, 0o1777] {
                fs::set_permissions(directory, fs::Permissions::from_mode(mode)).unwrap();
                assert!(validate_below(anchor, &store, owner).is_err());
            }
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
            validate_below(anchor, &store, owner).unwrap();
        }
        assert!(validate_below(anchor, &store, owner.wrapping_add(1)).is_err());
    }

    #[test]
    fn protected_walk_rejects_symlinks_files_and_escape() {
        let temporary = tempfile::tempdir().unwrap();
        let anchor = temporary.path();
        let owner = fs::metadata(anchor).unwrap().uid();
        let directory = anchor.join("directory");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let alias = anchor.join("alias");
        symlink(&directory, &alias).unwrap();
        assert!(validate_below(anchor, &alias, owner).is_err());
        assert!(validate_below(anchor, &alias.join("child"), owner).is_err());
        fs::write(anchor.join("file"), b"not a directory").unwrap();
        assert!(validate_below(anchor, &anchor.join("file"), owner).is_err());
        assert!(validate_below(anchor, &directory.join("../directory"), owner).is_err());
        assert!(validate_below(anchor, Path::new("/"), owner).is_err());
        assert!(validate(Path::new("relative/store")).is_err());
    }

    #[test]
    fn protected_constructor_rejects_unprivileged_caller_without_locking() {
        if crate::effective_uid() == 0 {
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        assert!(crate::ApplicationSlotStager::open_protected(temporary.path()).is_err());
        // The rejected privileged request must not retain the normal slot lock.
        crate::ApplicationSlotStager::open(temporary.path()).unwrap();
    }
}
