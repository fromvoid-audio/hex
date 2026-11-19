//! Revalidate the existing four-file ordinary-instance package directly.
//! Five fixed directory rows replace generic traversal/map construction. Extra
//! members, including empty directories, refuse; factory discovery retains its
//! existing inventory policy. OS iterator/entry/allocator internals are opaque,
//! distinct from the known reusable path and one returned name buffer below.
use super::*;

const FILES: [&str; 4] = [
    "hex-vst3-instance.app/Contents/Info.plist",
    INSTANCE_HELPER,
    "hex-vst3-instance.app/Contents/Resources/sdk-notices.json",
    "hex-vst3-instance.app/Contents/_CodeSignature/CodeResources",
];
/// The exact ordinary-instance role admits four map members. At most one
/// nonempty opaque BTreeMap node per admitted entry is needed by its retained
/// provenance copy; factory discovery uses its separate, larger policy.
pub const INSTANCE_INVENTORY_NODE_BOUND: usize = FILES.len();
const DIRECTORIES: [(&str, &[&str]); 5] = [
    (INSTANCE_APP, &["Contents"]),
    (
        "hex-vst3-instance.app/Contents",
        &["Info.plist", "MacOS", "Resources", "_CodeSignature"],
    ),
    (
        "hex-vst3-instance.app/Contents/MacOS",
        &["hex-vst3-instance"],
    ),
    (
        "hex-vst3-instance.app/Contents/Resources",
        &["sdk-notices.json"],
    ),
    (
        "hex-vst3-instance.app/Contents/_CodeSignature",
        &["CodeResources"],
    ),
];

/// Constructor-bounded helper revalidation scratch. No map nodes or traversal
/// queue are created. One file snapshot is separate from `data_bytes`; files
/// and directory enumeration do not overlap. Counts are opaque obligations,
/// never guessed OS/allocator byte sizes or process-wide handle observations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevalidationStorageBound {
    pub data_bytes: usize,
    pub file_data_bytes: usize,
    pub directory_iterator_lifetime_bound: usize,
    pub directory_entry_lifetime_bound: usize,
    pub file_reader_lifetime_bound: usize,
}

fn path_capacity(root: &Path) -> Option<usize> {
    if root.as_os_str().is_empty() || root.as_os_str().len() > MAX_PATH_BYTES {
        return None;
    }
    root.as_os_str()
        .len()
        .checked_add(1)?
        .checked_add(FILES.iter().map(|path| path.len()).max()?)
}
pub(super) fn storage_bound(root: &Path) -> Option<RevalidationStorageBound> {
    Some(RevalidationStorageBound {
        data_bytes: path_capacity(root)?.checked_add(MAX_PATH_BYTES)?,
        file_data_bytes: usize::try_from(MAX_BINARY).ok()?,
        directory_iterator_lifetime_bound: 1,
        directory_entry_lifetime_bound: 1,
        file_reader_lifetime_bound: 1,
    })
}

fn set_path(
    path: &mut PathBuf,
    root: &Path,
    relative: &str,
    capacity: usize,
) -> Result<(), ValidationError> {
    path.clear();
    path.push(root);
    path.push(relative);
    if path.capacity() != capacity {
        return Err("instance helper path allocation bound differs".into());
    }
    Ok(())
}

pub(super) fn verify(
    root: &Path,
    expected: &BTreeMap<String, String>,
) -> Result<(), ValidationError> {
    if expected.len() != FILES.len() || FILES.iter().any(|path| !expected.contains_key(*path)) {
        return Err("minimal instance helper inventory differs".into());
    }
    let capacity = path_capacity(root).ok_or("instance helper path bound")?;
    let mut path = PathBuf::new();
    path.try_reserve_exact(capacity)
        .map_err(|_| "instance helper path allocation refused")?;
    if path.capacity() != capacity {
        return Err("instance helper path allocation bound differs".into());
    }
    for (relative, names) in DIRECTORIES {
        set_path(&mut path, root, relative, capacity)?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| ValidationError::Io {
            context: "helper member missing",
            source,
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("linked or nonregular helper directory refused".into());
        }
        let mut seen = 0_u8;
        let mut count = 0;
        for entry in fs::read_dir(&path).map_err(|source| ValidationError::Io {
            context: "helper enumeration failed",
            source,
        })? {
            let entry = entry.map_err(|source| ValidationError::Io {
                context: "helper enumeration failed",
                source,
            })?;
            if count == names.len() {
                return Err("unexpected instance helper member".into());
            }
            let name = entry.file_name();
            if name.capacity() > MAX_PATH_BYTES {
                return Err("instance helper name allocation bound differs".into());
            }
            let index = names
                .iter()
                .position(|expected| name == std::ffi::OsStr::new(expected))
                .ok_or("unexpected instance helper member")?;
            let mask = 1_u8
                .checked_shl(u32::try_from(index).map_err(|_| "instance helper directory bound")?)
                .ok_or("instance helper directory bound")?;
            if seen & mask != 0 {
                return Err("duplicate instance helper member".into());
            }
            seen |= mask;
            count += 1;
        }
        if count != names.len() {
            return Err("instance helper member missing".into());
        }
    }
    for relative in FILES {
        set_path(&mut path, root, relative, capacity)?;
        let bytes = read_file(&path, MAX_BINARY)?;
        if !expected
            .get(relative)
            .is_some_and(|digest| hash_matches(&bytes, digest))
        {
            return Err("helper or build receipt changed during operation".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_digest_comparison_keeps_the_existing_sha256_and_exact_case() {
        for bytes in [b"".as_slice(), b"original helper", &[0, 255, 128]] {
            let expected = hash(bytes);
            assert!(hash_matches(bytes, &expected));
            assert!(!hash_matches(bytes, &expected.to_uppercase()));
            assert!(!hash_matches(bytes, &expected[..63]));
            assert!(!hash_matches(bytes, &format!("{expected}0")));
            assert!(!hash_matches(bytes, &"0".repeat(64)));
        }
    }

    #[test]
    fn fixed_directory_and_file_paths_reuse_one_bounded_buffer() {
        let root = Path::new("/original/é/helper root");
        let bound = storage_bound(root).unwrap();
        let capacity = path_capacity(root).unwrap();
        let mut path = PathBuf::new();
        path.try_reserve_exact(capacity).unwrap();
        for relative in DIRECTORIES
            .iter()
            .map(|(relative, _)| *relative)
            .chain(FILES)
        {
            set_path(&mut path, root, relative, capacity).unwrap();
            assert_eq!(path, root.join(relative));
            assert_eq!(path.capacity() + MAX_PATH_BYTES, bound.data_bytes);
        }
        assert_eq!(bound.file_data_bytes, MAX_BINARY as usize);
        assert_eq!(
            (
                bound.directory_iterator_lifetime_bound,
                bound.directory_entry_lifetime_bound,
                bound.file_reader_lifetime_bound
            ),
            (1, 1, 1)
        );
        assert!(storage_bound(Path::new("")).is_none());
        assert!(storage_bound(Path::new(&"r".repeat(MAX_PATH_BYTES + 1))).is_none());
    }
}
