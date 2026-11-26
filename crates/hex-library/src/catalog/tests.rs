use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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

#[test]
fn unchanged_rescan_skips_the_database_write_and_keeps_its_mtime() {
    let fixture = Fixture::new();
    let kick = fixture.0.join("lib/kick.wav");
    fs::create_dir_all(kick.parent().unwrap()).unwrap();
    write(&kick, b"kick");
    let roots = [fixture.0.join("lib")];
    let path = fixture.0.join("samples.db");
    let first = scan_samples(&roots);
    assert!(first.catalog.save(&path).unwrap(), "first save writes");
    // Backdate the cache so an unwanted rewrite is visible even on coarse clocks.
    let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(past)
        .unwrap();
    let second = scan_samples_with(
        &roots,
        ScanControl {
            previous: Some(&first.catalog),
            ..ScanControl::default()
        },
    );
    assert_eq!(second.catalog, first.catalog);
    assert!(
        !second.catalog.save(&path).unwrap(),
        "unchanged save skipped"
    );
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), past);

    // Same length, different bytes: the cache is rewritten, not trusted.
    let mut tampered = fs::read(&path).unwrap();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xff;
    fs::write(&path, &tampered).unwrap();
    assert!(second.catalog.save(&path).unwrap());
    assert_eq!(SampleCatalog::load(&path).unwrap(), second.catalog);

    // A library change writes again.
    write(&fixture.0.join("lib/snare.wav"), b"snare");
    let third = scan_samples(&roots);
    assert!(third.catalog.save(&path).unwrap());
    assert_eq!(SampleCatalog::load(&path).unwrap(), third.catalog);
}

// Linux filesystems support this arbitrary-byte filename; macOS filesystems
// may reject it before the scanner is called. The byte codec is tested below
// on every Unix target independently of filesystem filename restrictions.
#[cfg(target_os = "linux")]
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

#[test]
fn global_search_unions_index_and_folder_without_duplicate_or_lost_wavs() {
    let indexed = Fixture::new();
    let nested = indexed.file("nested/Utility.wav");
    let outside = Fixture::new();
    let outside_wav = outside.file("Utility-outside.wav");
    let project = outside.file("Utility.hex");
    let catalog = scan_samples(std::slice::from_ref(&indexed.0)).catalog;
    outside.file("deeper/Utility-not-recursively-searched.hex");
    let listing = crate::scan_directory(&outside.0).unwrap();
    let results = catalog.search_with_listing("utility", Some(&listing));
    assert_eq!(results.len(), 3);
    for path in [&nested, &outside_wav, &project] {
        assert_eq!(
            results.iter().filter(|entry| &entry.path == path).count(),
            1
        );
    }
    let overlapping = crate::scan_directory(indexed.0.join("nested")).unwrap();
    let deduplicated = catalog.search_with_listing("utility", Some(&overlapping));
    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].path, nested);
    assert!(
        catalog
            .search_with_listing("missing", Some(&listing))
            .is_empty()
    );
    assert_eq!(catalog.search_with_listing("utility", None).len(), 1);
    assert!(
        catalog
            .search_with_listing("", None)
            .iter()
            .all(|entry| entry.kind == EntryKind::WaveAudio)
    );
}

#[cfg(unix)]
#[test]
fn valid_native_path_scan_preserves_bytes_and_skips_symlink_cycles_and_aliases() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let path = fixture.file("Kick-🔊.WAV");
    symlink(&fixture.0, fixture.0.join("cycle")).unwrap();
    symlink(&path, fixture.0.join("alias.wav")).unwrap();
    let scan = scan_samples(std::slice::from_ref(&fixture.0));
    assert!(!scan.truncated);
    assert!(scan.errors.is_empty());
    assert_eq!(scan.catalog.entries().len(), 1);
    assert_eq!(scan.catalog.entries()[0].path, path);
    let db = fixture.0.join("native.db");
    scan.catalog.save(&db).unwrap();
    assert_eq!(SampleCatalog::load(&db).unwrap(), scan.catalog);
}

#[cfg(unix)]
#[test]
fn database_preserves_non_utf8_path_bytes_without_creating_that_sample_file() {
    use std::{
        ffi::OsString,
        os::unix::ffi::{OsStrExt, OsStringExt},
    };
    let fixture = Fixture::new();
    let path = fixture
        .0
        .join(OsString::from_vec(b"kick-\xff.wav".to_vec()));
    // Sample entries may refer to offline files; only the cache file is opened.
    let catalog = SampleCatalog {
        entries: vec![LibraryEntry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path: path.clone(),
            kind: EntryKind::WaveAudio,
        }],
        ..SampleCatalog::default()
    };
    let db = fixture.0.join("non-utf8.db");
    catalog.save(&db).unwrap();
    let restored = SampleCatalog::load(&db).unwrap();
    assert_eq!(restored, catalog);
    assert_eq!(
        restored.entries()[0].path.as_os_str().as_bytes(),
        path.as_os_str().as_bytes()
    );
}

#[test]
fn place_scoped_search_keeps_path_identity_and_excludes_other_roots_and_listings() {
    let fixture = Fixture::new();
    let left = fixture.0.join("left");
    let right = fixture.0.join("left-other");
    let a = fixture.file("left/nested/Kick.wav");
    let b = fixture.file("left-other/nested/Kick.wav");
    let project = fixture.file("left/Kick.hex");
    let foreign = fixture.file("left-other/Kick.hex");
    let catalog = scan_samples(std::slice::from_ref(&fixture.0)).catalog;
    let listing = crate::scan_directory(&left).unwrap();
    let scoped = catalog.search_with_listing_in("kick", Some(&left), Some(&listing));
    assert_eq!(scoped.len(), 2);
    assert!(scoped.iter().any(|entry| entry.path == a));
    assert!(scoped.iter().any(|entry| entry.path == project));
    assert!(
        !scoped
            .iter()
            .any(|entry| entry.path == b || entry.path == foreign)
    );
    let wrong_listing = crate::scan_directory(&right).unwrap();
    assert_eq!(
        catalog
            .search_with_listing_in("kick", Some(&left), Some(&wrong_listing))
            .iter()
            .map(|entry| &entry.path)
            .collect::<Vec<_>>(),
        [&a]
    );
    let all = catalog.search_with_listing("kick", Some(&listing));
    assert_eq!(all.len(), 3);
    assert!(all.iter().any(|entry| entry.path == a));
    assert!(all.iter().any(|entry| entry.path == b));
}

#[test]
fn place_correctness_listing_uses_indexed_terms_and_full_native_paths() {
    let fixture = Fixture::new();
    let root = fixture.0.join("kick");
    let wave = fixture.file("kick/drums.wav");
    let project = fixture.file("kick/drums.hex");
    let foreign = fixture.file("kick-other/drums.wav");
    let catalog = scan_samples(std::slice::from_ref(&fixture.0)).catalog;
    let listing = crate::scan_directory(&root).unwrap();
    for query in ["kick drums", "DRUMS  KICK", "  kick\t drums  "] {
        let scoped = catalog.search_with_listing_in(query, Some(&root), Some(&listing));
        assert_eq!(
            scoped.len(),
            2,
            "indexed and current-folder semantics differ for {query:?}"
        );
        assert!(scoped.iter().any(|entry| entry.path == wave));
        assert!(scoped.iter().any(|entry| entry.path == project));
        assert!(!scoped.iter().any(|entry| entry.path == foreign));
        let all = catalog.search_with_listing(query, Some(&listing));
        assert_eq!(
            all.len(),
            2,
            "existing All index semantics must remain intact"
        );
        assert!(all.iter().any(|entry| entry.path == wave));
        assert!(all.iter().any(|entry| entry.path == foreign));
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn fingerprint(catalog: &SampleCatalog, path: &Path) -> Option<Fingerprint> {
    catalog.record(path).and_then(|record| record.fingerprint)
}

#[test]
fn incremental_rescan_reuses_unchanged_records_and_refingerprints_changed_files() {
    let fixture = Fixture::new();
    let kick = fixture.0.join("drums/kick.wav");
    let snare = fixture.0.join("drums/snare.wav");
    write(&kick, b"kick body");
    write(&snare, b"snare");
    let roots = [fixture.0.clone()];
    let first = scan_samples(&roots);
    assert_eq!((first.fingerprinted, first.reused), (2, 0));
    let kick_print = fingerprint(&first.catalog, &kick).unwrap();
    assert_eq!(kick_print, Fingerprint::of_file(&kick).unwrap());
    assert_eq!(first.catalog.record(&kick).unwrap().size, 9);

    write(&snare, b"snare, but longer");
    let second = scan_samples_with(
        &roots,
        ScanControl {
            previous: Some(&first.catalog),
            ..ScanControl::default()
        },
    );
    assert_eq!((second.fingerprinted, second.reused), (1, 1));
    assert_eq!(fingerprint(&second.catalog, &kick), Some(kick_print));
    assert_ne!(
        fingerprint(&second.catalog, &snare),
        fingerprint(&first.catalog, &snare)
    );
    assert_eq!(
        fs::read(&kick).unwrap(),
        b"kick body",
        "scans are read-only"
    );
}

#[test]
fn cancellation_reports_a_partial_scan_and_progress_counts_entries() {
    let fixture = Fixture::new();
    fixture.file("a.wav");
    fixture.file("b.wav");
    let cancel = AtomicBool::new(true);
    let progress = AtomicUsize::new(0);
    let cancelled = scan_samples_with(
        std::slice::from_ref(&fixture.0),
        ScanControl {
            cancel: Some(&cancel),
            progress: Some(&progress),
            ..ScanControl::default()
        },
    );
    assert!(cancelled.cancelled);
    assert!(cancelled.catalog.entries().is_empty());
    cancel.store(false, std::sync::atomic::Ordering::Relaxed);
    let complete = scan_samples_with(
        std::slice::from_ref(&fixture.0),
        ScanControl {
            cancel: Some(&cancel),
            progress: Some(&progress),
            ..ScanControl::default()
        },
    );
    assert!(!complete.cancelled);
    assert_eq!(complete.catalog.entries().len(), 2);
    assert_eq!(progress.load(std::sync::atomic::Ordering::Relaxed), 2);
}

#[cfg(unix)]
#[test]
fn symlink_cycles_terminate_missing_roots_are_named_and_unreadable_files_stay_listed() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let nested = fixture.0.join("a/b");
    fs::create_dir_all(&nested).unwrap();
    symlink(&fixture.0, nested.join("loop")).unwrap();
    let locked = fixture.0.join("a/locked.wav");
    write(&locked, b"secret");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let missing = fixture.0.join("moved-away");
    let scan = scan_samples(&[missing.clone(), fixture.0.clone()]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(scan.missing_roots, [missing]);
    assert_eq!(scan.catalog.entries().len(), 1);
    // Root may read anything; only assert tolerance when the lock was effective.
    if scan.unreadable == 1 {
        assert_eq!(scan.catalog.record(&locked).unwrap().fingerprint, None);
        assert_eq!(scan.catalog.record(&locked).unwrap().size, 6);
    }
    assert!(!scan.truncated);
}

#[test]
fn records_round_trip_version_one_caches_load_and_corrupt_caches_are_preserved() {
    let fixture = Fixture::new();
    let kick = fixture.0.join("kick.wav");
    write(&kick, b"kick");
    let scan = scan_samples(std::slice::from_ref(&fixture.0));
    let db = fixture.0.join("samples.db");
    scan.catalog.save(&db).unwrap();
    let loaded = SampleCatalog::load(&db).unwrap();
    assert_eq!(loaded, scan.catalog);
    assert!(fingerprint(&loaded, &kick).is_some());

    // A version 1 cache (paths only) still loads; records come from the next scan.
    #[cfg(windows)]
    let raw: Vec<u8> = {
        use std::os::windows::ffi::OsStrExt;
        kick.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    };
    #[cfg(not(windows))]
    let raw = kick.as_os_str().as_encoded_bytes().to_vec();
    let mut version_one = b"hexsmp1".to_vec();
    version_one.push(fs::read(&db).unwrap()[7]);
    version_one.extend_from_slice(&1_u32.to_le_bytes());
    version_one.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    version_one.extend_from_slice(&raw);
    fs::write(&db, &version_one).unwrap();
    let old = SampleCatalog::load(&db).unwrap();
    assert_eq!(old.entries(), scan.catalog.entries());
    assert!(old.record(&kick).is_none());

    // An unknown record tag is refused; preserving moves the bytes aside intact.
    let mut bad = fs::read(&db).unwrap();
    bad[6] = b'2';
    bad.push(9);
    fs::write(&db, &bad).unwrap();
    assert!(SampleCatalog::load(&db).is_err());
    let preserved = SampleCatalog::preserve_corrupt(&db).unwrap().unwrap();
    assert_eq!(fs::read(&preserved).unwrap(), bad);
    assert!(!db.exists());
    assert_eq!(SampleCatalog::load(&db).unwrap(), SampleCatalog::default());
    assert_eq!(SampleCatalog::preserve_corrupt(&db).unwrap(), None);
}

#[test]
fn candidates_rank_fingerprint_then_name_and_size_then_name() {
    let fixture = Fixture::new();
    let renamed = fixture.0.join("a/renamed.wav");
    let same_size = fixture.0.join("b/Kick.WAV");
    let other_size = fixture.0.join("c/kick.wav");
    write(&renamed, b"original kick");
    write(&same_size, b"different kic");
    write(&other_size, b"kick");
    fixture.file("d/snare.wav");
    let catalog = scan_samples(std::slice::from_ref(&fixture.0)).catalog;
    let original = Fingerprint::of_file(&renamed).unwrap();
    let matches = catalog.find_candidates(&MissingSample {
        file_name: std::ffi::OsStr::new("kick.wav"),
        size: Some(13),
        fingerprint: Some(original),
    });
    assert_eq!(
        matches
            .iter()
            .map(|found| (found.path.clone(), found.kind))
            .collect::<Vec<_>>(),
        [
            (renamed, MatchKind::Fingerprint),
            (same_size, MatchKind::NameAndSize),
            (other_size.clone(), MatchKind::Name),
        ]
    );
    let by_name =
        catalog.find_candidates(&MissingSample::named(Path::new("/gone/KICK.wav")).unwrap());
    assert_eq!(by_name.len(), 2);
    assert!(by_name.iter().all(|found| found.kind == MatchKind::Name));
    assert!(
        catalog
            .find_candidates(&MissingSample::named(Path::new("/gone/none.wav")).unwrap())
            .is_empty()
    );
    assert!(MissingSample::named(Path::new("/")).is_none());
}

#[test]
fn relocating_a_root_remaps_records_by_relative_path_for_incremental_reuse() {
    let fixture = Fixture::new();
    let old_root = fixture.0.join("old");
    let new_root = fixture.0.join("new");
    write(&old_root.join("drums/kick.wav"), b"kick");
    write(&fixture.0.join("other/pad.wav"), b"pad");
    let roots = [old_root.clone(), fixture.0.join("other")];
    let mut catalog = scan_samples(&roots).catalog;
    let print = fingerprint(&catalog, &old_root.join("drums/kick.wav")).unwrap();
    fs::rename(&old_root, &new_root).unwrap();
    catalog.relocate_root(&old_root, &new_root);
    let moved = new_root.join("drums/kick.wav");
    assert_eq!(fingerprint(&catalog, &moved), Some(print));
    assert!(catalog.entries().iter().any(|entry| entry.path == moved));
    assert!(
        catalog
            .entries()
            .iter()
            .any(|entry| entry.path == fixture.0.join("other/pad.wav"))
    );
    let rescan = scan_samples_with(
        &[new_root.clone(), fixture.0.join("other")],
        ScanControl {
            previous: Some(&catalog),
            ..ScanControl::default()
        },
    );
    assert_eq!((rescan.reused, rescan.fingerprinted), (2, 0));
    assert_eq!(fingerprint(&rescan.catalog, &moved), Some(print));
}

#[test]
fn catalog_indexes_persists_and_finds_rf64_aiff_and_flac_relink_candidates() {
    let fixture = Fixture::new();
    let rf64 = fixture.0.join("stems/long.rf64");
    let aiff = fixture.0.join("stems/pad.aiff");
    let flac = fixture.0.join("stems/drums.flac");
    fs::create_dir_all(fixture.0.join("stems")).unwrap();
    write(&rf64, b"synthetic-rf64-bytes");
    write(&aiff, b"synthetic-aiff-bytes");
    write(&flac, b"synthetic-flac-bytes");

    let scan = scan_samples(std::slice::from_ref(&fixture.0));
    let names: Vec<&str> = scan
        .catalog
        .entries()
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["drums.flac", "long.rf64", "pad.aiff"]);
    assert_eq!(scan.fingerprinted, 3);

    // Save and reload catalog database cleanly with non-WAV extensions
    let db_path = fixture.0.join("samples.db");
    assert!(scan.catalog.save(&db_path).unwrap());
    let reloaded = SampleCatalog::load(&db_path).unwrap();
    assert_eq!(reloaded, scan.catalog);

    // Relink lookup finds moved non-WAV formats by name & fingerprint
    let flac_fp = fingerprint(&reloaded, &flac).unwrap();
    let candidates = reloaded.find_candidates(&MissingSample {
        file_name: std::ffi::OsStr::new("drums.flac"),
        size: Some(b"synthetic-flac-bytes".len() as u64),
        fingerprint: Some(flac_fp),
    });
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].path, flac);
}

fn minimal_wav(sample_rate: u32, channels: u16, bit_depth: u16, frames: u32) -> Vec<u8> {
    let block_align = channels * (bit_depth / 8);
    let data_bytes = frames * u32::from(block_align);
    let byte_rate = sample_rate * u32::from(block_align);
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bit_depth.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    out
}

fn minimal_flac(sample_rate: u32, channels: u16, bit_depth: u16, frames: u64) -> Vec<u8> {
    let mut flac = vec![0_u8; 42];
    flac[0..4].copy_from_slice(b"fLaC");
    flac[4] = 0x80;
    flac[7] = 34;
    let ch_m1 = u32::from(channels - 1);
    let bps_m1 = u32::from(bit_depth - 1);
    flac[18] = (sample_rate >> 12) as u8;
    flac[19] = (sample_rate >> 4) as u8;
    flac[20] = (((sample_rate & 0x0f) << 4) | (ch_m1 << 1) | (bps_m1 >> 4)) as u8;
    flac[21] = (((bps_m1 & 0x0f) << 4) | ((frames >> 32) as u32 & 0x0f)) as u8;
    flac[22..26].copy_from_slice(&(frames as u32).to_be_bytes());
    flac
}

#[test]
fn hexsmp2_migrates_to_hexsmp3_and_preserves_audio_headers_tags_and_collections() {
    let fixture = Fixture::new();
    let kick = fixture.0.join("drums/kick.wav");
    let pad = fixture.0.join("synths/pad.flac");
    write(&kick, &minimal_wav(48_000, 2, 24, 24_000));
    write(&pad, &minimal_flac(96_000, 2, 24, 192_000));

    let roots = [fixture.0.clone()];
    let mut scan = scan_samples(&roots);
    assert_eq!((scan.fingerprinted, scan.reused), (2, 0));

    let kick_meta = scan.catalog.audio_metadata(&kick).unwrap();
    assert_eq!(kick_meta.sample_rate, 48_000);
    assert_eq!(kick_meta.channels, 2);
    assert_eq!(kick_meta.bit_depth, 24);
    assert_eq!(kick_meta.frame_count, 24_000);
    assert_eq!(kick_meta.duration_ms(), 500);
    assert_eq!(kick_meta.codec, AudioCodec::WavPcm);

    let pad_meta = scan.catalog.audio_metadata(&pad).unwrap();
    assert_eq!(pad_meta.sample_rate, 96_000);
    assert_eq!(pad_meta.duration_ms(), 2_000);
    assert_eq!(pad_meta.codec, AudioCodec::Flac);

    // Attach persistent tags and collections, validate bounds/normalization
    assert!(scan.catalog.add_tag(&kick, "  Punchy ").unwrap());
    assert!(!scan.catalog.add_tag(&kick, "punchy").unwrap());
    assert!(scan.catalog.add_tag(&kick, "drums").unwrap());
    assert!(scan.catalog.add_tag(&pad, "ambient").unwrap());
    assert!(scan.catalog.add_tag(&pad, "").is_err());
    assert!(scan.catalog.create_collection("Favorites").unwrap());
    assert!(scan.catalog.add_to_collection("favorites", &kick).unwrap());
    assert!(scan.catalog.add_to_collection("favorites", &pad).unwrap());

    let db = fixture.0.join("catalog.db");
    assert!(scan.catalog.save(&db).unwrap());
    let raw = fs::read(&db).unwrap();
    assert_eq!(&raw[..7], b"hexsmp3");
    assert!(
        !scan.catalog.save(&db).unwrap(),
        "identical bytes skip write"
    );

    let loaded = SampleCatalog::load(&db).unwrap();
    assert_eq!(loaded, scan.catalog);
    assert_eq!(loaded.all_tags(), vec!["ambient", "drums", "punchy"]);
    assert_eq!(loaded.collections(), vec!["favorites"]);
    assert_eq!(loaded.collections_for(&kick), vec!["favorites"]);

    // Incremental rescan reuses records with zero extra fingerprints and preserves tags/collections
    let rescan = scan_samples_with(
        &roots,
        ScanControl {
            previous: Some(&loaded),
            ..ScanControl::default()
        },
    );
    assert_eq!((rescan.reused, rescan.fingerprinted), (2, 0));
    assert_eq!(rescan.catalog, loaded);

    // Relocate root remaps both records and persistent tags/collections
    let mut relocated = loaded.clone();
    let moved_root = fixture.0.join("moved-root");
    relocated.relocate_root(&fixture.0, &moved_root);
    let moved_kick = moved_root.join("drums/kick.wav");
    assert!(relocated.tags_for(&moved_kick).unwrap().contains("punchy"));
    assert!(
        relocated
            .collection_members("favorites")
            .unwrap()
            .contains(&moved_kick)
    );

    // Genuine hexsmp2 file loads cleanly and upgrades to hexsmp3 on save
    let kick_rec = loaded.record(&kick).unwrap();
    #[cfg(windows)]
    let raw_kick_path: Vec<u8> = {
        use std::os::windows::ffi::OsStrExt;
        kick.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    };
    #[cfg(not(windows))]
    let raw_kick_path = kick.as_os_str().as_encoded_bytes().to_vec();
    let mut v2 = b"hexsmp2".to_vec();
    v2.push(raw[7]);
    v2.extend_from_slice(&1_u32.to_le_bytes());
    v2.extend_from_slice(&(raw_kick_path.len() as u32).to_le_bytes());
    v2.extend_from_slice(&raw_kick_path);
    v2.push(2); // RECORD_FINGERPRINTED
    v2.extend_from_slice(&kick_rec.size.to_le_bytes());
    v2.extend_from_slice(&kick_rec.modified_ns.to_le_bytes());
    v2.extend_from_slice(&kick_rec.fingerprint.unwrap().value().to_le_bytes());
    let v2_db = fixture.0.join("legacy-v2.db");
    fs::write(&v2_db, &v2).unwrap();
    let migrated = SampleCatalog::load(&v2_db).unwrap();
    assert_eq!(migrated.entries().len(), 1);
    assert_eq!(
        migrated.record(&kick).unwrap().fingerprint,
        kick_rec.fingerprint
    );
    assert_eq!(migrated.audio_metadata(&kick), None);
    assert!(migrated.save(&v2_db).unwrap());
    assert_eq!(&fs::read(&v2_db).unwrap()[..7], b"hexsmp3");

    // Newer/corrupt hexsmp4 catalog is refused without modifying disk bytes
    let mut future = raw.clone();
    future[6] = b'4';
    let future_db = fixture.0.join("future-v4.db");
    fs::write(&future_db, &future).unwrap();
    assert!(SampleCatalog::load(&future_db).is_err());
    assert_eq!(fs::read(&future_db).unwrap(), future);
}

#[test]
fn metadata_query_tags_collections_and_sorting_accurate_across_10k_entries() {
    let count = 12_000_usize;
    let mut catalog = SampleCatalog::default();
    let root = PathBuf::from("/synthetic-library");
    for i in 0..count {
        let codec = match i % 3 {
            0 => AudioCodec::WavPcm,
            1 => AudioCodec::Flac,
            _ => AudioCodec::AiffPcm,
        };
        let ext = match codec {
            AudioCodec::WavPcm => "wav",
            AudioCodec::Flac => "flac",
            _ => "aiff",
        };
        let sample_rate = if i % 4 == 0 { 96_000 } else { 48_000 };
        let channels = if i % 2 == 0 { 2 } else { 1 };
        let bit_depth = if i % 5 == 0 { 24 } else { 16 };
        let frame_count = u64::from(sample_rate) * ((i as u64 % 10) + 1);
        let name = format!("sample_{i:05}.{ext}");
        let path = root.join(format!("pack_{}/{}", i % 16, name));
        catalog.records.insert(
            path.clone(),
            SampleRecord {
                size: 4096 + i as u64,
                modified_ns: 1_000_000,
                fingerprint: Some(Fingerprint::from_value(i as u64 + 1)),
                audio: Some(AudioHeaderMetadata {
                    sample_rate,
                    channels,
                    bit_depth,
                    frame_count,
                    codec,
                }),
                audio_inspected: true,
            },
        );
        if i % 100 == 0 {
            catalog.add_tag(&path, "featured").unwrap();
        }
        if i % 200 == 0 {
            catalog.add_to_collection("live-set", &path).unwrap();
        }
        catalog.entries.push(LibraryEntry {
            name,
            path,
            kind: EntryKind::WaveAudio,
        });
    }
    sort_entries(&mut catalog.entries, true);
    catalog.rebuild_search_index();

    // Structured query across 12,000 entries: tag + codec + sample_rate + duration sort
    let matches = catalog.query(&CatalogQuery {
        tag: Some("featured"),
        codec: Some(AudioCodec::Flac),
        sample_rate: Some(96_000),
        channels: Some(2),
        sort: CatalogSort::DurationDesc,
        ..CatalogQuery::default()
    });
    // i % 100 == 0 (already implies i % 4 == 0 and i % 2 == 0) AND i % 3 == 1 => i ≡ 100 (mod 300)
    let expected = (0..count).filter(|i| i % 100 == 0 && i % 3 == 1).count();
    assert_eq!(matches.len(), expected);
    assert!(matches.windows(2).all(|w| {
        catalog.audio_metadata(&w[0].path).unwrap().duration_ms()
            >= catalog.audio_metadata(&w[1].path).unwrap().duration_ms()
    }));

    // Inline query tokens work identically through `filtered()`
    let inline = catalog.filtered("tag:featured col:live-set codec:wav rate:96000 ch:2", None);
    let expected_inline = (0..count).filter(|i| i % 200 == 0 && i % 3 == 0).count();
    assert_eq!(inline.len(), expected_inline);
    assert!(inline.iter().all(|e| {
        catalog
            .collection_members("live-set")
            .unwrap()
            .contains(&e.path)
    }));
}

#[test]
fn v2_migrated_records_populate_audio_metadata_on_next_scan_without_re_reading_v3_unparseable() {
    let fixture = Fixture::new();
    let valid_wav = fixture.0.join("drums/kick.wav");
    let raw_bytes = fixture.0.join("drums/opaque.wav");
    write(&valid_wav, &minimal_wav(48_000, 2, 24, 48_000));
    write(&raw_bytes, b"not-a-valid-audio-header");

    let roots = [fixture.0.clone()];
    let initial = scan_samples(&roots);
    assert_eq!(initial.fingerprinted, 2);
    assert!(initial.catalog.audio_metadata(&valid_wav).is_some());
    assert_eq!(initial.catalog.audio_metadata(&raw_bytes), None);

    // Save v3 and reload: both the valid wav and the v3-inspected unparseable file
    // must be reused on incremental rescan (0 re-fingerprints).
    let v3_db = fixture.0.join("v3.db");
    initial.catalog.save(&v3_db).unwrap();
    let v3_loaded = SampleCatalog::load(&v3_db).unwrap();
    let v3_rescan = scan_samples_with(
        &roots,
        ScanControl {
            previous: Some(&v3_loaded),
            ..ScanControl::default()
        },
    );
    assert_eq!((v3_rescan.reused, v3_rescan.fingerprinted), (2, 0));

    // Construct a genuine legacy hexsmp2 cache with matching size/mtime/fingerprint
    // for both unchanged files: upgrading v2 -> v3 must inspect headers on the next
    // scan instead of permanently starving metadata.
    let v3_raw = fs::read(&v3_db).unwrap();
    let encode_path = |p: &Path| -> Vec<u8> {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            p.as_os_str()
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect()
        }
        #[cfg(not(windows))]
        {
            p.as_os_str().as_encoded_bytes().to_vec()
        }
    };
    let mut v2 = b"hexsmp2".to_vec();
    v2.push(v3_raw[7]);
    v2.extend_from_slice(&2_u32.to_le_bytes());
    for path in [&valid_wav, &raw_bytes] {
        let rec = initial.catalog.record(path).unwrap();
        let raw_path = encode_path(path);
        v2.extend_from_slice(&(raw_path.len() as u32).to_le_bytes());
        v2.extend_from_slice(&raw_path);
        v2.push(2); // RECORD_FINGERPRINTED (v2)
        v2.extend_from_slice(&rec.size.to_le_bytes());
        v2.extend_from_slice(&rec.modified_ns.to_le_bytes());
        v2.extend_from_slice(&rec.fingerprint.unwrap().value().to_le_bytes());
    }
    let v2_db = fixture.0.join("v2.db");
    fs::write(&v2_db, &v2).unwrap();
    let migrated = SampleCatalog::load(&v2_db).unwrap();
    assert!(!migrated.record(&valid_wav).unwrap().audio_inspected);
    assert_eq!(migrated.audio_metadata(&valid_wav), None);

    // Next incremental scan re-inspects both legacy v2 records and populates audio metadata.
    let upgraded = scan_samples_with(
        &roots,
        ScanControl {
            previous: Some(&migrated),
            ..ScanControl::default()
        },
    );
    assert_eq!((upgraded.reused, upgraded.fingerprinted), (0, 2));
    assert_eq!(
        upgraded
            .catalog
            .audio_metadata(&valid_wav)
            .unwrap()
            .sample_rate,
        48_000
    );
    assert!(upgraded.catalog.record(&raw_bytes).unwrap().audio_inspected);
    assert_eq!(upgraded.catalog.audio_metadata(&raw_bytes), None);

    // Subsequent scan reuses both (including the v3-inspected unparseable file).
    let steady = scan_samples_with(
        &roots,
        ScanControl {
            previous: Some(&upgraded.catalog),
            ..ScanControl::default()
        },
    );
    assert_eq!((steady.reused, steady.fingerprinted), (2, 0));
}

#[test]
fn mutations_enforce_absolute_paths_and_bounds_and_invalid_filters_return_empty() {
    let fixture = Fixture::new();
    let root_a = fixture.0.join("a");
    let root_b = fixture.0.join("b");
    let sample_a = root_a.join("kick.wav");
    let sample_b = root_b.join("kick.wav");
    write(&sample_a, &minimal_wav(48_000, 2, 24, 24_000));
    write(&sample_b, &minimal_wav(48_000, 2, 24, 24_000));
    let mut catalog = scan_samples(std::slice::from_ref(&fixture.0)).catalog;

    // 1. Relative and oversized paths are refused at mutation time (never producing unloadable saves).
    assert!(
        catalog
            .add_tag(Path::new("relative/kick.wav"), "drums")
            .is_err()
    );
    assert!(
        catalog
            .add_to_collection("favs", Path::new("relative/kick.wav"))
            .is_err()
    );
    let oversized = fixture.0.join("x".repeat(MAX_PATH_BYTES + 1));
    assert!(catalog.add_tag(&oversized, "drums").is_err());
    assert!(catalog.add_to_collection("favs", &oversized).is_err());

    // 2. Relocating root_a onto root_b merges tags capped at MAX_TAGS_PER_SAMPLE and stays save/loadable.
    for i in 0..20 {
        catalog
            .add_tag(&sample_a, &format!("a-tag-{i:02}"))
            .unwrap();
        catalog
            .add_tag(&sample_b, &format!("b-tag-{i:02}"))
            .unwrap();
    }
    // Relative relocate_root target is ignored safely.
    catalog.relocate_root(&root_a, Path::new("relative/target"));
    assert!(catalog.tags_for(&sample_a).is_some());

    catalog.relocate_root(&root_a, &root_b);
    assert_eq!(
        catalog.tags_for(&sample_b).unwrap().len(),
        MAX_TAGS_PER_SAMPLE
    );
    let db = fixture.0.join("bounded.db");
    catalog.save(&db).unwrap();
    assert_eq!(SampleCatalog::load(&db).unwrap(), catalog);

    // 3. Invalid q.tag / q.collection return empty results rather than silently dropping the filter.
    for bad in ["", "   ", "bad\0label", &"z".repeat(MAX_TAG_BYTES + 1)] {
        assert!(
            catalog
                .query(&CatalogQuery {
                    tag: Some(bad),
                    ..CatalogQuery::default()
                })
                .is_empty(),
            "invalid tag {bad:?} must return empty"
        );
        assert!(
            catalog
                .query(&CatalogQuery {
                    collection: Some(bad),
                    ..CatalogQuery::default()
                })
                .is_empty(),
            "invalid collection {bad:?} must return empty"
        );
    }

    // 4. search_with_listing_in shares inline tokenization (e.g. invalid rate: or tag: filter).
    let listing = crate::scan_directory(&root_b).unwrap();
    assert!(
        catalog
            .search_with_listing_in("rate:notanumber", None, Some(&listing))
            .is_empty()
    );
    assert!(
        catalog
            .search_with_listing_in("tag:nonexistent", Some(&root_b), Some(&listing))
            .is_empty()
    );
    let tagged = catalog.search_with_listing_in("tag:a-tag-00 kick", Some(&root_b), Some(&listing));
    assert_eq!(tagged.len(), 1);
    assert_eq!(tagged[0].path, sample_b);
}

#[test]
fn schwartzian_metadata_sorts_preserve_exact_order_missing_placement_and_path_tiebreaks() {
    struct SortFixture {
        path: &'static str,
        size: Option<u64>,
        audio: Option<(u32, u16, u64)>,
    }
    let mut catalog = SampleCatalog::default();
    let specs = [
        SortFixture {
            path: "/lib/b_short_48k_24.wav",
            size: Some(4_000),
            audio: Some((48_000, 24, 500)),
        },
        SortFixture {
            path: "/lib/a_short_48k_24.wav",
            size: Some(4_000),
            audio: Some((48_000, 24, 500)),
        },
        SortFixture {
            path: "/lib/c_long_96k_16.wav",
            size: Some(9_000),
            audio: Some((96_000, 16, 2_000)),
        },
        SortFixture {
            path: "/lib/d_mid_44k_32.wav",
            size: Some(2_000),
            audio: Some((44_100, 32, 1_000)),
        },
        SortFixture {
            path: "/lib/e_record_no_audio.wav",
            size: Some(12_000),
            audio: None,
        },
        SortFixture {
            path: "/lib/f_no_record.wav",
            size: None,
            audio: None,
        },
    ];
    for spec in specs {
        let path = PathBuf::from(spec.path);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if let Some(size) = spec.size {
            let audio = spec.audio.map(|(sample_rate, bit_depth, duration_ms)| {
                let frame_count = (u64::from(sample_rate) * duration_ms) / 1_000;
                AudioHeaderMetadata {
                    sample_rate,
                    channels: 2,
                    bit_depth,
                    frame_count,
                    codec: AudioCodec::WavPcm,
                }
            });
            catalog.records.insert(
                path.clone(),
                SampleRecord {
                    size,
                    modified_ns: 1,
                    fingerprint: Some(Fingerprint::from_value(1)),
                    audio,
                    audio_inspected: true,
                },
            );
        }
        catalog.entries.push(LibraryEntry {
            name,
            path,
            kind: EntryKind::WaveAudio,
        });
    }
    sort_entries(&mut catalog.entries, true);
    catalog.rebuild_search_index();

    let paths_for = |sort: CatalogSort| -> Vec<String> {
        catalog
            .query(&CatalogQuery {
                sort,
                ..CatalogQuery::default()
            })
            .into_iter()
            .map(|e| e.path.to_string_lossy().into_owned())
            .collect()
    };

    assert_eq!(
        paths_for(CatalogSort::DurationAsc),
        [
            "/lib/a_short_48k_24.wav",
            "/lib/b_short_48k_24.wav",
            "/lib/d_mid_44k_32.wav",
            "/lib/c_long_96k_16.wav",
            "/lib/e_record_no_audio.wav",
            "/lib/f_no_record.wav",
        ]
    );
    assert_eq!(
        paths_for(CatalogSort::DurationDesc),
        [
            "/lib/c_long_96k_16.wav",
            "/lib/d_mid_44k_32.wav",
            "/lib/a_short_48k_24.wav",
            "/lib/b_short_48k_24.wav",
            "/lib/e_record_no_audio.wav",
            "/lib/f_no_record.wav",
        ]
    );
    assert_eq!(
        paths_for(CatalogSort::SampleRateAsc),
        [
            "/lib/d_mid_44k_32.wav",
            "/lib/a_short_48k_24.wav",
            "/lib/b_short_48k_24.wav",
            "/lib/c_long_96k_16.wav",
            "/lib/e_record_no_audio.wav",
            "/lib/f_no_record.wav",
        ]
    );
    assert_eq!(
        paths_for(CatalogSort::SampleRateDesc),
        [
            "/lib/c_long_96k_16.wav",
            "/lib/a_short_48k_24.wav",
            "/lib/b_short_48k_24.wav",
            "/lib/d_mid_44k_32.wav",
            "/lib/e_record_no_audio.wav",
            "/lib/f_no_record.wav",
        ]
    );
    assert_eq!(
        paths_for(CatalogSort::BitDepthDesc),
        [
            "/lib/d_mid_44k_32.wav",
            "/lib/a_short_48k_24.wav",
            "/lib/b_short_48k_24.wav",
            "/lib/c_long_96k_16.wav",
            "/lib/e_record_no_audio.wav",
            "/lib/f_no_record.wav",
        ]
    );
    assert_eq!(
        paths_for(CatalogSort::SizeDesc),
        [
            "/lib/e_record_no_audio.wav",
            "/lib/c_long_96k_16.wav",
            "/lib/a_short_48k_24.wav",
            "/lib/b_short_48k_24.wav",
            "/lib/d_mid_44k_32.wav",
            "/lib/f_no_record.wav",
        ]
    );
}

#[test]
fn prefolded_search_index_stays_in_sync_across_load_relocate_and_retain() {
    let fixture = Fixture::new();
    let old_root = fixture.0.join("PackAlpha");
    let keep_root = fixture.0.join("KeepBeta");
    let moved_root = fixture.0.join("PackRenamed");
    let kick = old_root.join("ΣΟΣ_Kick.WAV");
    let pad = keep_root.join("WarmPad.wav");
    write(&kick, &minimal_wav(48_000, 2, 16, 4_800));
    write(&pad, &minimal_wav(48_000, 2, 16, 4_800));

    let scanned = scan_samples(&[old_root.clone(), keep_root.clone()]).catalog;
    assert_eq!(scanned.folded_paths.len(), scanned.entries.len());
    assert_eq!(scanned.filtered("packalpha σος", None).len(), 1);

    let db = fixture.0.join("folded.db");
    assert!(scanned.save(&db).unwrap());
    let mut loaded = SampleCatalog::load(&db).unwrap();
    assert_eq!(loaded.folded_paths.len(), loaded.entries.len());
    assert_eq!(loaded.filtered("packalpha kick", None).len(), 1);

    loaded.relocate_root(&old_root, &moved_root);
    assert_eq!(loaded.folded_paths.len(), loaded.entries.len());
    assert!(loaded.filtered("packalpha", None).is_empty());
    let moved = loaded.filtered("packrenamed σος", None);
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].path, moved_root.join("ΣΟΣ_Kick.WAV"));

    loaded.retain_roots(std::slice::from_ref(&keep_root));
    assert_eq!(loaded.folded_paths.len(), loaded.entries.len());
    assert!(loaded.filtered("packrenamed", None).is_empty());
    assert_eq!(loaded.filtered("warmpad", None).len(), 1);
}

#[test]
fn streaming_save_compare_handles_multi_chunk_noop_mutation_truncation_and_growth() {
    let fixture = Fixture::new();
    let mut catalog = SampleCatalog::default();
    // ~600 entries produce > 40 KiB, spanning multiple 16 KiB compare chunks.
    for i in 0..600_usize {
        let path = PathBuf::from(format!(
            "/synthetic/streaming-compare/bank-{:02}/sample-{i:04}-long-path-segment.wav",
            i % 16
        ));
        catalog.records.insert(
            path.clone(),
            SampleRecord {
                size: 1024 + i as u64,
                modified_ns: 42,
                fingerprint: Some(Fingerprint::from_value(i as u64 + 1)),
                audio: Some(AudioHeaderMetadata {
                    sample_rate: 48_000,
                    channels: 2,
                    bit_depth: 24,
                    frame_count: 48_000,
                    codec: AudioCodec::WavPcm,
                }),
                audio_inspected: true,
            },
        );
        catalog.entries.push(LibraryEntry {
            name: format!("sample-{i:04}-long-path-segment.wav"),
            path,
            kind: EntryKind::WaveAudio,
        });
    }
    sort_entries(&mut catalog.entries, true);
    catalog.rebuild_search_index();

    let db = fixture.0.join("multi-chunk.db");
    assert!(catalog.save(&db).unwrap());
    let encoded = fs::read(&db).unwrap();
    assert!(
        encoded.len() > 32 * 1024,
        "fixture must span multiple compare chunks, got {} bytes",
        encoded.len()
    );
    assert!(
        !catalog.save(&db).unwrap(),
        "exact multi-chunk match is a no-op"
    );

    // Flip a byte in the second chunk (offset > 16 KiB): must detect and rewrite.
    let mut mutated = encoded.clone();
    mutated[20_000] ^= 0x55;
    fs::write(&db, &mutated).unwrap();
    assert!(catalog.save(&db).unwrap());
    assert_eq!(fs::read(&db).unwrap(), encoded);

    // Truncate by 1 byte: must detect and rewrite.
    fs::write(&db, &encoded[..encoded.len() - 1]).unwrap();
    assert!(catalog.save(&db).unwrap());
    assert_eq!(fs::read(&db).unwrap(), encoded);

    // Append 1 trailing byte: must detect and rewrite.
    let mut grown = encoded.clone();
    grown.push(0);
    fs::write(&db, &grown).unwrap();
    assert!(catalog.save(&db).unwrap());
    assert_eq!(fs::read(&db).unwrap(), encoded);
}
