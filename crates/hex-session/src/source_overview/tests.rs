use super::*;
const SHORT_BIN_BYTES: usize = std::mem::size_of::<StereoPeak>();
fn facts() -> OriginalSourceFacts {
    OriginalSourceFacts::new(1, [7; 32]).unwrap()
}
fn asset() -> AssetId {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
    let hex_project::Created::Asset(id) = editor
        .execute(hex_project::Command::AddAsset {
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 1,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    id
}
fn metadata_budget() -> usize {
    OverviewBudget::standard()
        .unwrap()
        .usage()
        .max_metadata_bytes
}
#[test]
fn literal_short_mono_normalized_and_nondivisible_stereo_cover_every_source_frame() {
    let budget = OverviewBudget::standard().unwrap();
    let mono = [[-0.5; 2], [0.25; 2], [1.0; 2]];
    let summary =
        OriginalSourceOverview::prepare(&budget, &mono, 8000, facts(), &AtomicBool::new(false))
            .unwrap();
    assert_eq!(summary.source_frames(), 3);
    assert_eq!(summary.sample_rate(), 8000);
    assert_eq!(summary.content(), facts());
    assert_eq!(summary.bin_count(), 3);
    assert_eq!(
        summary.peaks().copied().collect::<Vec<_>>(),
        mono.map(|f| f.map(|s| Peak { min: s, max: s })).to_vec()
    );
    let mut stereo = vec![[0.25, -0.5]; 8195];
    stereo[0] = [-0.75, 0.125];
    stereo[2] = [0.875, -1.0];
    stereo[8194] = [-1.0, 0.875];
    let summary =
        OriginalSourceOverview::prepare(&budget, &stereo, 16000, facts(), &AtomicBool::new(false))
            .unwrap();
    let bins = summary.peaks().copied().collect::<Vec<_>>();
    assert_eq!(bins.len(), 4096);
    assert_eq!(summary.bin_range(0), Some(0..3));
    assert_eq!(summary.bin_range(2), Some(6..9));
    assert_eq!(summary.bin_range(3), Some(9..11));
    assert_eq!(summary.bin_range(4095), Some(8193..8195));
    assert_eq!(summary.bin_range(4096), None);
    // 8195 = first3 bins of3 frames, then4093 bins of2 frames.
    assert_eq!(
        bins[0],
        [
            Peak {
                min: -0.75,
                max: 0.875
            },
            Peak {
                min: -1.0,
                max: 0.125
            }
        ]
    );
    assert_eq!(
        bins[1],
        [
            Peak {
                min: 0.25,
                max: 0.25
            },
            Peak {
                min: -0.5,
                max: -0.5
            }
        ]
    );
    assert_eq!(
        bins[4095],
        [
            Peak {
                min: -1.0,
                max: 0.25
            },
            Peak {
                min: -0.5,
                max: 0.875
            }
        ]
    );
}
#[test]
fn detailed_range_queries_keep_transients_and_charge_only_actual_capacity() {
    let budget = OverviewBudget::standard().unwrap();
    let mut frames = vec![[0.125, -0.25]; 8195];
    frames[4097] = [-1.0, 0.875];
    frames[8194] = [0.75, -0.9];
    let summary =
        OriginalSourceOverview::prepare(&budget, &frames, 48000, facts(), &AtomicBool::new(false))
            .unwrap();
    assert_eq!(budget.usage().bin_capacity_bytes, 4096 * 16);
    assert_eq!(budget.usage().max_bin_capacity_bytes, 256 * 1024 * 1024);
    assert_eq!(
        summary.peaks_for_frames(4097, 4098),
        Some([
            Peak {
                min: -1.0,
                max: 0.125
            },
            Peak {
                min: -0.25,
                max: 0.875
            }
        ])
    );
    assert_eq!(
        summary.peaks_for_frames(8193, 8195),
        Some([
            Peak {
                min: 0.125,
                max: 0.75
            },
            Peak {
                min: -0.9,
                max: -0.25
            }
        ])
    );
    assert_eq!(
        summary.peaks_for_frames(0, u64::MAX),
        Some([
            Peak {
                min: -1.0,
                max: 0.75
            },
            Peak {
                min: -0.9,
                max: 0.875
            }
        ])
    );
    assert!(summary.peaks_for_frames(8195, u64::MAX).is_none());
    assert!(summary.peaks_for_frames(2, 2).is_none());
    drop(summary);
    assert_eq!(budget.usage().bin_capacity_bytes, 0);
}

#[test]
fn exact_and_one_under_budget_retained_table_and_last_backing_drop() {
    let metadata = metadata_budget();
    let too_small = OverviewBudget::with_limits(SHORT_BIN_BYTES - 1, metadata).unwrap();
    let initial = too_small.usage();
    assert!(
        OriginalSourceOverview::prepare(
            &too_small,
            &[[0.5; 2]],
            8000,
            facts(),
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(too_small.usage(), initial);
    let budget = OverviewBudget::with_limits(SHORT_BIN_BYTES, metadata).unwrap();
    let base = budget.usage();
    let overview = OriginalSourceOverview::prepare(
        &budget,
        &[[0.5; 2]],
        8000,
        facts(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let current = OriginalSourceOverviews::default()
        .inserted(&budget, asset(), overview)
        .unwrap();
    let retained_view = current.clone();
    assert!(retained_view.shares_storage_with(&current));
    let full = budget.usage();
    assert_eq!(full.bin_capacity_bytes, SHORT_BIN_BYTES);
    drop(current);
    assert_eq!(
        budget.usage(),
        full,
        "a retained old view keeps the capacity charged"
    );
    assert!(
        OriginalSourceOverview::prepare(
            &budget,
            &[[0.25; 2]],
            8000,
            facts(),
            &AtomicBool::new(false)
        )
        .is_err()
    );
    drop(retained_view);
    assert_eq!(budget.usage(), base);
    let retry = OriginalSourceOverview::prepare(
        &budget,
        &[[0.25; 2]],
        8000,
        facts(),
        &AtomicBool::new(false),
    )
    .unwrap();
    drop(retry);
    assert_eq!(budget.usage(), base);
    let needed = full.metadata_bytes;
    let under = OverviewBudget::with_limits(SHORT_BIN_BYTES, needed - 1).unwrap();
    let baseline = under.usage();
    let overview = OriginalSourceOverview::prepare(
        &under,
        &[[0.5; 2]],
        8000,
        facts(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        OriginalSourceOverviews::default()
            .inserted(&under, asset(), overview)
            .is_err()
    );
    assert_eq!(under.usage(), baseline);
    let exact = OverviewBudget::with_limits(SHORT_BIN_BYTES, needed).unwrap();
    let overview = OriginalSourceOverview::prepare(
        &exact,
        &[[0.5; 2]],
        8000,
        facts(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let table = OriginalSourceOverviews::default()
        .inserted(&exact, asset(), overview)
        .unwrap();
    assert_eq!(exact.usage().metadata_bytes, needed);
    drop(table);
}
#[test]
fn malformed_cancelled_and_duplicate_source_construction_releases_staged_only() {
    let budget = OverviewBudget::standard().unwrap();
    let base = budget.usage();
    for frames in [&[][..], &[[f32::NAN, 0.0]][..], &[[0.0, f32::INFINITY]][..]] {
        assert!(
            OriginalSourceOverview::prepare(
                &budget,
                frames,
                8000,
                facts(),
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert_eq!(budget.usage(), base);
    }
    assert!(
        OriginalSourceOverview::prepare(
            &budget,
            &[[0.5; 2]],
            8000,
            facts(),
            &AtomicBool::new(true)
        )
        .is_err()
    );
    assert_eq!(budget.usage(), base);
    let id = asset();
    let first = OriginalSourceOverview::prepare(
        &budget,
        &[[0.5; 2]],
        8000,
        facts(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let table = OriginalSourceOverviews::default()
        .inserted(&budget, id, first)
        .unwrap();
    let before = budget.usage();
    let staged = OriginalSourceOverview::prepare(
        &budget,
        &[[0.25; 2]],
        8000,
        facts(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(table.inserted(&budget, id, staged).is_err());
    let alien = OverviewBudget::standard().unwrap();
    let alien_base = alien.usage();
    let alien_source = OriginalSourceOverview::prepare(
        &alien,
        &[[0.25; 2]],
        8000,
        facts(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        OriginalSourceOverviews::default()
            .inserted(&budget, id, alien_source)
            .is_err()
    );
    assert_eq!(alien.usage(), alien_base);
    assert_eq!(budget.usage(), before);
    assert_eq!(
        table.get(id).unwrap().peaks().next().unwrap(),
        &[Peak { min: 0.5, max: 0.5 }; 2]
    );
}

#[test]
fn mid_scan_cancellation_is_polled_within_4096_frames_and_preserves_old_table_budget() {
    let budget = OverviewBudget::standard().unwrap();
    let overview = OriginalSourceOverview::prepare(
        &budget,
        &[[0.5; 2]],
        8000,
        facts(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let id = asset();
    let table = OriginalSourceOverviews::default()
        .inserted(&budget, id, overview)
        .unwrap();
    let before = budget.usage();
    let frames = vec![[0.25; 2]; 16_384];
    let polls = std::cell::Cell::new(0_usize);
    let last_frame = std::cell::Cell::new(0_usize);
    let refused =
        OriginalSourceOverview::prepare_cancellable(&budget, &frames, 8000, facts(), |processed| {
            polls.set(polls.get() + 1);
            last_frame.set(processed);
            polls.get() == 3
        });
    assert!(refused.is_err());
    assert_eq!(last_frame.get(), 4096);
    assert_eq!(budget.usage(), before);
    assert_eq!(
        table.get(id).unwrap().peaks().next().unwrap(),
        &[Peak { min: 0.5, max: 0.5 }; 2]
    );
}

#[test]
fn replacement_table_accounts_for_old_and_new_row_backing_before_commit() {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 1).unwrap();
    let mut ids = Vec::new();
    for name in ["a.wav", "b.wav"] {
        let hex_project::Created::Asset(id) = editor
            .execute(hex_project::Command::AddAsset {
                path: name.into(),
                sample_rate: 48000,
                frames: 1,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        ids.push(id);
    }
    let build = |budget: &OverviewBudget| {
        let first = OriginalSourceOverview::prepare(
            budget,
            &[[0.5; 2]],
            48000,
            facts(),
            &AtomicBool::new(false),
        )
        .unwrap();
        OriginalSourceOverviews::default()
            .inserted(budget, ids[1], first)
            .unwrap()
    };
    let generous = OverviewBudget::standard().unwrap();
    let old = build(&generous);
    let second = OriginalSourceOverview::prepare(
        &generous,
        &[[0.25; 2]],
        48000,
        facts(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let candidate = old.inserted(&generous, ids[0], second).unwrap();
    let need = generous.usage().metadata_bytes;
    assert_eq!(generous.usage().bin_capacity_bytes, 2 * SHORT_BIN_BYTES);
    assert_eq!(candidate.len(), 2);
    assert_eq!(
        candidate.get(ids[0]).unwrap().peaks().next().unwrap(),
        &[Peak {
            min: 0.25,
            max: 0.25
        }; 2]
    );
    assert_eq!(old.len(), 1);
    let alien = OverviewBudget::standard().unwrap();
    let alien_base = alien.usage();
    let alien_source = OriginalSourceOverview::prepare(
        &alien,
        &[[0.25; 2]],
        48000,
        facts(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(old.inserted(&alien, ids[0], alien_source).is_err());
    assert_eq!(alien.usage(), alien_base);
    for (limit, success) in [(need - 1, false), (need, true)] {
        let budget = OverviewBudget::with_limits(2 * SHORT_BIN_BYTES, limit).unwrap();
        let old = build(&budget);
        let before = budget.usage();
        let second = OriginalSourceOverview::prepare(
            &budget,
            &[[0.25; 2]],
            48000,
            facts(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let candidate = old.inserted(&budget, ids[0], second);
        assert_eq!(candidate.is_ok(), success);
        if success {
            assert_eq!(budget.usage().metadata_bytes, need);
        } else {
            assert_eq!(
                budget.usage(),
                before,
                "failed replacement frees staged backing and preserves old table"
            );
        }
        assert_eq!(
            old.get(ids[1]).unwrap().peaks().next().unwrap(),
            &[Peak { min: 0.5, max: 0.5 }; 2]
        );
    }
}
