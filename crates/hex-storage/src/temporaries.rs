//! Regression coverage for preserving unclaimed interrupted-save files.

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-temporaries-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("test directory");
        Self(path)
    }

    fn file(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, b"copy").expect("fixture file");
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn save_and_replace_preserve_old_matching_user_files() {
    let dir = Dir::new();
    let target = dir.0.join("song.hex");
    let project = hex_project::Project::new(48_000).expect("project");
    let mut retained = Vec::new();
    for name in [".hex-pending-1-1", ".hex-r-1-2"] {
        let path = dir.file(name);
        fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(SystemTime::now() - Duration::from_secs(7200)))
            .expect("age fixture");
        retained.push(path);
    }
    crate::save_new(&target, &project).expect("save new");
    for path in &retained {
        assert_eq!(fs::read(path).expect("preserve old user file"), b"copy");
    }

    let again = dir.file(".hex-r-1-3");
    fs::File::options()
        .write(true)
        .open(&again)
        .and_then(|file| file.set_modified(SystemTime::now() - Duration::from_secs(7200)))
        .expect("age fixture");
    let (_, token) = crate::load_with_token(&target).expect("token");
    crate::replace_existing(&target, &token, &project).expect("replace");
    assert_eq!(
        fs::read(&again).expect("preserve another user file"),
        b"copy"
    );
    for path in &retained {
        assert_eq!(
            fs::read(path).expect("preserve across replacement"),
            b"copy"
        );
    }
    assert!(crate::load(&target).is_ok());
    assert!(crate::recovery_path(&target).expect("sidecar").exists());
}
