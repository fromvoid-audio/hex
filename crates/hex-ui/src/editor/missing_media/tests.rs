use super::*;
use hex_session::service::missing_media::{SearchOutcome, SearchStop};

fn assets(count: usize) -> Vec<hex_project::AssetId> {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
    (0..count)
        .map(|_| {
            let hex_project::Created::Asset(id) = editor
                .execute(hex_project::Command::AddAsset {
                    path: "x.wav".into(),
                    sample_rate: 48_000,
                    frames: 1,
                })
                .unwrap()
            else {
                panic!("asset")
            };
            id
        })
        .collect()
}

fn rows(ids: &[hex_project::AssetId]) -> Vec<Row> {
    ids.iter()
        .map(|&asset| Row {
            media: MissingMedia {
                asset,
                expected_path: PathBuf::from("/gone/a.wav"),
                source_frames: 1,
                source_sample_rate: 48_000,
                device_original: false,
                usages: Vec::new(),
                usage_count: 0,
            },
            resolution: Resolution::Missing,
        })
        .collect()
}

#[test]
fn search_resolves_unique_lists_choices_and_flags_mismatch_without_overwriting_found() {
    let ids = assets(4);
    let mut rows = rows(&ids);
    rows[3].resolution = Resolution::Found("/kept.wav".into(), CandidateFit::Matches);
    let outcome = SearchOutcome {
        candidates: BTreeMap::from([
            (ids[0], vec![("/one.wav".into(), CandidateFit::Matches)]),
            (
                ids[1],
                vec![
                    ("/x/two.wav".into(), CandidateFit::Matches),
                    ("/y/two.wav".into(), CandidateFit::Matches),
                ],
            ),
            (
                ids[2],
                vec![("/three.wav".into(), CandidateFit::Mismatch("length".into()))],
            ),
            (ids[3], vec![("/other.wav".into(), CandidateFit::Matches)]),
        ]),
        scanned: 9,
        stopped: Some(SearchStop::Cancelled),
        ..SearchOutcome::default()
    };
    assert_eq!(apply_search(&mut rows, &outcome), 1);
    assert_eq!(
        rows[0].resolution,
        Resolution::Found("/one.wav".into(), CandidateFit::Matches)
    );
    assert_eq!(
        rows[1].resolution,
        Resolution::Choices(vec!["/x/two.wav".into(), "/y/two.wav".into()])
    );
    assert_eq!(
        rows[2].resolution,
        Resolution::Mismatch("/three.wav".into(), "length".into())
    );
    assert_eq!(
        rows[3].resolution,
        Resolution::Found("/kept.wav".into(), CandidateFit::Matches)
    );
    assert_eq!(
        search_summary(&outcome, 1),
        "searched 9 entries · found 1 · cancelled"
    );
}

#[test]
fn locate_flags_mismatch_and_applies_relative_mappings_to_unresolved_rows() {
    let ids = assets(3);
    let mut rows = rows(&ids);
    let mismatch = CandidateFit::Mismatch("sample rate".into());
    assert_eq!(
        apply_locate(&mut rows, 0, "/bad.wav".into(), mismatch, Vec::new()),
        0
    );
    assert!(matches!(rows[0].resolution, Resolution::Mismatch(..)));
    rows[2].resolution = Resolution::Found("/kept.wav".into(), CandidateFit::Matches);
    let mapped = vec![(ids[1], "/new/b.wav".into()), (ids[2], "/new/c.wav".into())];
    let fit = CandidateFit::Unverified("adpcm".into());
    assert_eq!(
        apply_locate(&mut rows, 0, "/new/a.wav".into(), fit.clone(), mapped),
        1
    );
    assert_eq!(
        rows[0].resolution,
        Resolution::Found("/new/a.wav".into(), fit)
    );
    assert_eq!(
        rows[1].resolution,
        Resolution::Found("/new/b.wav".into(), CandidateFit::Matches)
    );
    assert_eq!(
        rows[2].resolution,
        Resolution::Found("/kept.wav".into(), CandidateFit::Matches)
    );
}
