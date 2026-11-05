//! Opt-in whole-query release profile. Generated paths never touch the filesystem.
use super::*;
use std::{cmp::Ordering, hint::black_box, time::Instant};

fn legacy_order(a: &LibraryEntry, b: &LibraryEntry) -> Ordering {
    let rank = |kind| if kind == EntryKind::Directory { 0 } else { 1 };
    rank(a.kind)
        .cmp(&rank(b.kind))
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.name.cmp(&b.name))
        .then_with(|| a.path.cmp(&b.path))
}

#[test]
#[ignore = "opt-in optimized control-side query profile"]
fn profile_library_queries() {
    for count in [1_000, 10_000, 50_000] {
        let mut original: Vec<_> = (0..count)
            .map(|i| {
                let prefix = ["Kick", "kick", "ΣΟΣ", "İ", "snare", "pad", "Bass"][i % 7];
                let name = format!("{prefix}-{:05}.wav", (i * 7919) % count);
                LibraryEntry {
                    path: PathBuf::from(format!("/generated/library/group-{}/{name}", i % 13)),
                    name,
                    kind: EntryKind::WaveAudio,
                }
            })
            .collect();
        original.sort_by(legacy_order);
        for sorted in [true, false] {
            let mut entries = original.clone();
            if !sorted {
                entries.rotate_left(count / 3);
                entries.reverse();
            }
            let catalog = SampleCatalog {
                entries,
                ..SampleCatalog::default()
            };
            for query in ["", "kick", "group-1 pad", "missing"] {
                let terms: Vec<_> = query.split_whitespace().collect();
                let mut expected: Vec<_> = original
                    .iter()
                    .filter(|e| {
                        let folded = e.path.to_string_lossy().to_lowercase();
                        terms.iter().all(|t| folded.contains(t))
                    })
                    .cloned()
                    .collect();
                expected.sort_by(legacy_order);
                let mut times = Vec::new();
                for iteration in 0..48 {
                    let start = Instant::now();
                    let actual =
                        black_box(catalog.search_with_listing_in(black_box(query), None, None));
                    let elapsed = start.elapsed().as_secs_f64() * 1e6;
                    assert_eq!(actual, expected);
                    if iteration >= 8 {
                        times.push(elapsed);
                    }
                }
                times.sort_by(f64::total_cmp);
                println!(
                    "LIBRARY_CSV,{count},{sorted},{query},{},{:.3},{:.3},{:.3}",
                    expected.len(),
                    times[20],
                    times[38],
                    times[39]
                );
            }
        }
    }
    println!("library query profile: all exact results checked");
}

#[test]
fn cached_order_preserves_legacy_unicode_native_paths_and_stable_ties() {
    let names = ["kick", "Kick", "ΣΟΣ", "ΣΟς", "İ", "i\u{307}", "", "same"];
    let mut seed = 0x123456789_u64;
    for count in [0, 1, 2, 3, 17, 256, 1024] {
        let mut entries: Vec<_> = (0..count)
            .map(|i| LibraryEntry {
                name: names[i % names.len()].to_owned(),
                path: PathBuf::from(format!("/original/{}/{}", i % 11, i % 19)),
                kind: [
                    EntryKind::Directory,
                    EntryKind::WaveAudio,
                    EntryKind::Project,
                ][i % 3],
            })
            .collect();
        #[cfg(unix)]
        if let Some(e) = entries.first_mut() {
            use std::os::unix::ffi::OsStringExt;
            e.path = std::ffi::OsString::from_vec(b"/original/\xff.wav".to_vec()).into();
        }
        for _ in 0..16 {
            for i in (1..count).rev() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                entries.swap(i, (seed as usize) % (i + 1));
            }
            for with_path in [false, true] {
                let mut expected = entries.clone();
                expected.sort_by(|a, b| {
                    let rank = |k| if k == EntryKind::Directory { 0 } else { 1 };
                    rank(a.kind)
                        .cmp(&rank(b.kind))
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                        .then_with(|| a.name.cmp(&b.name))
                        .then_with(|| {
                            if with_path {
                                a.path.cmp(&b.path)
                            } else {
                                Ordering::Equal
                            }
                        })
                });
                let mut actual = entries.clone();
                crate::sort_entries(&mut actual, with_path);
                assert_eq!(actual, expected);
            }
        }
    }
}

#[test]
fn folder_equal_names_keep_input_order_while_catalog_uses_native_paths() {
    let entries: Vec<_> = [
        (EntryKind::Project, "/b"),
        (EntryKind::WaveAudio, "/a"),
        (EntryKind::Project, "/a"),
    ]
    .into_iter()
    .map(|(kind, path)| LibraryEntry {
        name: "same".into(),
        path: path.into(),
        kind,
    })
    .collect();
    let mut folder = entries.clone();
    crate::sort_entries(&mut folder, false);
    assert_eq!(folder, entries);
    let mut catalog = entries.clone();
    crate::sort_entries(&mut catalog, true);
    assert_eq!(
        catalog,
        vec![entries[1].clone(), entries[2].clone(), entries[0].clone()]
    );
}

#[test]
fn query_normalization_matches_legacy_global_place_and_native_path_semantics() {
    fn filtered(entries: &[LibraryEntry], query: &str, source: Option<&Path>) -> Vec<LibraryEntry> {
        let query = query.to_lowercase();
        let terms: Vec<_> = query.split_whitespace().collect();
        entries
            .iter()
            .filter(|e| source.is_none_or(|s| e.path.starts_with(s)))
            .filter(|e| {
                let path = e.path.to_string_lossy().to_lowercase();
                terms.iter().all(|t| path.contains(t))
            })
            .cloned()
            .collect()
    }
    let make = |name: &str, path: &str, kind| LibraryEntry {
        name: name.into(),
        path: path.into(),
        kind,
    };
    let mut entries = vec![
        make("Kick.WAV", "/ROOT/Drums/Kick.WAV", EntryKind::WaveAudio),
        make("ΣΟΣ.wav", "/ROOT/ΟΣ/ΣΟΣ.wav", EntryKind::WaveAudio),
        make("İ.wav", "/ROOT/İ/i\u{307}.wav", EntryKind::WaveAudio),
        make("kick", "/other/Drums/Kick.wav", EntryKind::WaveAudio),
        make("upper", "/root/DRUMS/KICK.wav", EntryKind::WaveAudio),
        make("a  b.wav", "/ROOT/a  b.wav", EntryKind::WaveAudio),
    ];
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut e = make("invalid.wav", "/unused", EntryKind::WaveAudio);
        e.path = std::ffi::OsString::from_vec(b"/ROOT/\xffKick.wav".to_vec()).into();
        entries.push(e);
    }
    entries.sort_by(legacy_order);
    let catalog = SampleCatalog {
        entries,
        ..SampleCatalog::default()
    };
    let listing = DirectoryListing {
        directory: "/ROOT".into(),
        entries: vec![
            catalog.entries[0].clone(),
            make(
                "Kick Drums.hex",
                "/ROOT/Drums/project.hex",
                EntryKind::Project,
            ),
            make("Drums Kick", "/ROOT/more/Kick", EntryKind::Directory),
            make("ΣΟΣ", "/ROOT/ΟΣ/listing", EntryKind::Directory),
            make("Kick", "/ROOT-sibling/Kick", EntryKind::Directory),
            make("İ", "/ROOT/İ", EntryKind::Directory),
        ],
        enumerated_entries: 6,
        truncated: false,
    };
    for query in [
        "",
        " ",
        "  \t",
        "KICK",
        "kick drums",
        "Drums Kick",
        "a  b",
        "ΣΟΣ",
        "ος",
        "İ",
        "i\u{307}",
        "�kick",
        "missing",
    ] {
        for source in [
            None,
            Some(Path::new("/ROOT")),
            Some(Path::new("/root")),
            Some(Path::new("/absent")),
        ] {
            assert_eq!(
                catalog.filtered(query, source),
                filtered(&catalog.entries, query, source)
            );
            for listing in [None, Some(&listing)] {
                let mut expected = filtered(&catalog.entries, query, source);
                if let Some(listing) = listing {
                    let folded = query.to_lowercase();
                    let mut seen: BTreeSet<_> = expected.iter().map(|e| e.path.clone()).collect();
                    for e in listing.entries.iter().take(MAX_RETAINED_ENTRIES) {
                        if !source.is_none_or(|s| e.path.starts_with(s)) {
                            continue;
                        }
                        let matches = if source.is_some() {
                            !filtered(std::slice::from_ref(e), query, source).is_empty()
                        } else {
                            e.name.to_lowercase().contains(&folded)
                        };
                        if matches && seen.insert(e.path.clone()) {
                            expected.push(e.clone());
                        }
                    }
                }
                expected.sort_by(legacy_order);
                assert_eq!(
                    catalog.search_with_listing_in(query, source, listing),
                    expected,
                    "query={query:?} source={source:?}"
                );
            }
        }
    }
}

/// Opt-in release scan-throughput profile over a generated temporary folder:
/// 2,000 WAV-named files of 128 KiB in 20 subfolders (256 MiB). The first scan
/// fingerprints every file (page cache warm from generation); the second reuses
/// every record through size/mtime. Run with
/// `cargo test -p hex-library --release -- --ignored profile_scan_throughput --nocapture`.
#[test]
#[ignore = "opt-in optimized scan throughput profile; writes 256 MiB to the temp dir"]
fn profile_scan_throughput() {
    const FILES: usize = 2_000;
    const BYTES: usize = 128 * 1024;
    let root = std::env::temp_dir().join(format!("hex-scan-profile-{}", std::process::id()));
    let body: Vec<u8> = (0..BYTES).map(|i| (i % 251) as u8).collect();
    for i in 0..FILES {
        let path = root.join(format!("group-{:02}/sample-{i:05}.wav", i % 20));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = body.clone();
        bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
        fs::write(path, bytes).unwrap();
    }
    let roots = [root.clone()];
    let start = Instant::now();
    let first = scan_samples(&roots);
    let full = start.elapsed();
    assert_eq!(first.fingerprinted, FILES);
    let start = Instant::now();
    let second = scan_samples_with(
        &roots,
        ScanControl {
            previous: Some(&first.catalog),
            ..ScanControl::default()
        },
    );
    let incremental = start.elapsed();
    assert_eq!(second.reused, FILES);
    let _ = fs::remove_dir_all(&root);
    println!(
        "scan {FILES} files x {BYTES} B: fingerprint {:.1} ms ({:.0} files/s), incremental {:.1} ms ({:.0} files/s)",
        full.as_secs_f64() * 1e3,
        FILES as f64 / full.as_secs_f64(),
        incremental.as_secs_f64() * 1e3,
        FILES as f64 / incremental.as_secs_f64(),
    );
}
