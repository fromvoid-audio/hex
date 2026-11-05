use super::*;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hex-helper-inventory-{}-{stamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("helper")).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn exact_member_limit_hashes_all_files_and_an_extra_member_refuses() {
    let directory = Directory::new();
    for index in 0..MAX_MEMBERS {
        fs::write(directory.0.join(format!("helper/{index}")), b"original").unwrap();
    }
    let files = inventory(&directory.0, "helper").unwrap();
    assert_eq!(files.len(), MAX_MEMBERS);
    assert!(files.values().all(|value| value == &hash(b"original")));
    fs::write(directory.0.join("helper/extra"), b"extra").unwrap();
    assert!(
        inventory(&directory.0, "helper")
            .unwrap_err()
            .contains("member/depth bound")
    );
}

#[test]
fn deepest_legal_member_is_retained_and_deeper_paths_refuse() {
    let directory = Directory::new();
    let mut path = directory.0.join("helper");
    for _ in 1..MAX_DEPTH {
        path.push("d");
        fs::create_dir(&path).unwrap();
    }
    fs::write(path.join("leaf"), b"leaf").unwrap();
    assert_eq!(inventory(&directory.0, "helper").unwrap().len(), 1);
    fs::create_dir(path.join("deeper")).unwrap();
    fs::write(path.join("deeper/leaf"), b"deep").unwrap();
    assert!(
        inventory(&directory.0, "helper")
            .unwrap_err()
            .contains("member/depth bound")
    );
}

#[test]
fn directory_and_file_relative_names_share_the_declared_byte_bound() {
    let root = Path::new("root");
    let longest = "a".repeat(MAX_PATH_BYTES);
    assert_eq!(
        relative_member(root, &root.join(&longest)).unwrap(),
        longest
    );
    assert!(relative_member(root, &root.join(format!("{longest}x"))).is_err());
    assert!(relative_member(root, root).is_err());
    assert!(relative_member(root, Path::new("outside/member")).is_err());
}
