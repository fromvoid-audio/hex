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
            let catalog = SampleCatalog { entries };
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
