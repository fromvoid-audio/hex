use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-catalog-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, []).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn combines_nested_overlapping_roots_and_filters_terms_and_sources() {
    let fixture = Fixture::new();
    let kick = fixture.file("Drums/Kick.WAV");
    fixture.file("Synth/Kick.wav");
    fixture.file(".hidden/Secret.wav");
    fixture.file("Drums/.secret.wav");
    fixture.file("Drums/not-a-sample.hex");
    let roots = [
        fixture.0.clone(),
        fixture.0.join("Drums"),
        fixture.0.clone(),
    ];
    let scan = scan_samples(&roots);
    assert!(!scan.truncated);
    assert!(scan.errors.is_empty());
    assert_eq!(scan.catalog.entries().len(), 2);
    let matches = scan.catalog.filtered("KICK drums", None);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].path, kick);
    assert_eq!(
        scan.catalog
            .filtered("kick", Some(&fixture.0.join("Synth")))
            .len(),
        1
    );
    assert!(scan.catalog.filtered("missing", None).is_empty());
}

#[test]
fn rescans_reconcile_removed_renamed_and_new_files_and_report_offline_roots() {
    let fixture = Fixture::new();
    let old = fixture.file("old.wav");
    let roots = [fixture.0.join("offline"), fixture.0.clone()];
    assert_eq!(scan_samples(&roots).catalog.entries().len(), 1);
    fs::rename(old, fixture.0.join("renamed.wav")).unwrap();
    let removed = fixture.file("removed.wav");
    let before = scan_samples(&roots);
    fs::remove_file(removed).unwrap();
    fixture.file("new.wav");
    let after = scan_samples(&roots);
    assert_eq!(after.errors.len(), 1);
    assert_eq!(
        after
            .catalog
            .entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["new.wav", "renamed.wav"]
    );
    assert_ne!(before.catalog, after.catalog);
}

#[test]
fn database_round_trips_and_refuses_malformed_oversized_and_future_data() {
    let fixture = Fixture::new();
    fixture.file("nested/Kick.WAV");
    let path = fixture.0.join("samples.db");
    assert_eq!(
        SampleCatalog::load(&path).unwrap(),
        SampleCatalog::default()
    );
    let catalog = scan_samples(std::slice::from_ref(&fixture.0)).catalog;
    catalog.save(&path).unwrap();
    assert_eq!(SampleCatalog::load(&path).unwrap(), catalog);
    let valid = fs::read(&path).unwrap();
    for corrupt in [
        b"hexsmp9u".to_vec(),
        valid[..valid.len() - 1].to_vec(),
        [valid.as_slice(), b"extra"].concat(),
    ] {
        fs::write(&path, &corrupt).unwrap();
        assert!(SampleCatalog::load(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), corrupt);
    }
    let file = fs::File::create(&path).unwrap();
    file.set_len(MAX_CATALOG_BYTES as u64 + 1).unwrap();
    assert!(SampleCatalog::load(&path).is_err());
    // This derived database is deliberately recoverable by a fresh scan.
    catalog.save(&path).unwrap();
    assert_eq!(SampleCatalog::load(&path).unwrap(), catalog);
    let blocked = fixture.file("not-a-folder").join("samples.db");
    assert!(catalog.save(&blocked).is_err());
    assert_eq!(SampleCatalog::load(&path).unwrap(), catalog);
}

#[cfg(unix)]
#[test]
fn preserves_native_path_bytes_and_skips_symlink_cycles_and_file_aliases() {
    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs::symlink},
    };
    let fixture = Fixture::new();
    let path = fixture
        .0
        .join(OsString::from_vec(b"kick-\xff.wav".to_vec()));
    fs::write(&path, []).unwrap();
    symlink(&fixture.0, fixture.0.join("cycle")).unwrap();
    symlink(&path, fixture.0.join("alias.wav")).unwrap();
    let catalog = scan_samples(std::slice::from_ref(&fixture.0)).catalog;
    assert_eq!(catalog.entries().len(), 1);
    assert_eq!(catalog.entries()[0].path, path);
    let db = fixture.0.join("native.db");
    catalog.save(&db).unwrap();
    assert_eq!(SampleCatalog::load(&db).unwrap(), catalog);
}

#[test]
fn bounded_depth_and_roots_are_visible_and_cache_excludes_removed_roots() {
    let fixture = Fixture::new();
    let nested = format!("{}sample.wav", "d/".repeat(MAX_SCAN_DEPTH + 1));
    fixture.file(&nested);
    fixture.file("visible.wav");
    let scan = scan_samples(std::slice::from_ref(&fixture.0));
    assert!(scan.truncated);
    assert_eq!(scan.catalog.entries().len(), 1);
    let roots = vec![fixture.0.clone(); MAX_ROOTS + 1];
    assert!(scan_samples(&roots).truncated);
    let mut catalog = scan.catalog;
    catalog.retain_roots(&[fixture.0.join("elsewhere")]);
    assert!(catalog.entries().is_empty());
}
