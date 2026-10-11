//! Public API regression for the external missing-attribute errno mapping.
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::time::{SystemTime, UNIX_EPOCH};
use xattr::FileExt;

#[test]
fn absent_attribute_is_none_and_missing_path_is_an_error() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("hex-xattr-{}-{unique}", std::process::id()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    // Query through both syscall paths; a missing attribute must not surface as
    // an error, and an unrelated ENOENT must never turn into Ok(None).
    let by_path = xattr::get(&path, "user.hex_missing_attribute");
    let by_fd = file.get_xattr("user.hex_missing_attribute");
    drop(file);
    fs::remove_file(&path).unwrap();
    assert_eq!(by_path.unwrap(), None);
    assert_eq!(by_fd.unwrap(), None);
    assert_eq!(
        xattr::get(&path, "user.hex_missing_attribute")
            .unwrap_err()
            .kind(),
        ErrorKind::NotFound
    );
}
