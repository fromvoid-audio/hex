use super::*;
use hex_engine::{Audio, SineTable};
use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry, NativeInstrumentRegistry};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;

#[path = "../../../hex-engine/examples/support/latency_fixture.rs"]
mod latency_fixture;

#[test]
fn latent_bounce_matches_reference_across_render_partitions() {
    let directory =
        std::env::temp_dir().join(format!("hex-bounce-partitions-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let samples: Vec<_> = (0..557)
        .map(|frame| {
            let value = ((frame % 41) as f32 - 20.0) / 64.0;
            [value, -value]
        })
        .collect();
    let definitions: [&dyn NativeDeviceDefinition; 2] = [
        &latency_fixture::Definition,
        &hex_lattice::conformance::owned_delay::DEFINITION,
    ];
    let effects = NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = NativeInstrumentRegistry::new(&[]).unwrap();
    let mut revisions = Vec::new();
    for device_count in [0, 1, 40] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 128).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "source".into(),
                path: "memory.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 557,
                playback_frames: 557,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import")
        };
        for _ in 0..device_count {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: latency_fixture::Definition.default_state().unwrap(),
                })
                .unwrap();
        }
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_lattice::conformance::owned_delay::state(37).unwrap(),
            })
            .unwrap();
        let media = BTreeMap::from([(asset, Audio::new(48_000, samples.clone()).unwrap())]);
        let revision = PreparedRevision::prepare_with_test_registries(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
            &effects,
            &instruments,
        )
        .unwrap();
        assert_eq!(
            revision.arrangement().main_latency_frames(),
            device_count * 7
        );
        revisions.push(Arc::new(revision));
    }
    for block in [16, 127, 256] {
        for (start, frames, tails) in [(29, 310, false), (0, 557, true)] {
            let mut reference = None;
            for (index, revision) in revisions.iter().enumerate() {
                let path = directory.join(format!("{block}-{start}-{tails}-{index}.wav"));
                let report = render_arrangement_bounce_partitioned(
                    &path,
                    Arc::clone(revision),
                    None,
                    start,
                    frames,
                    tails,
                    &ExportControl::default(),
                    None,
                    block,
                )
                .unwrap();
                if tails {
                    assert!(report.frames >= 594, "the musical tail must survive");
                } else {
                    assert_eq!(report.frames, frames);
                }
                let bytes = std::fs::read(path).unwrap();
                let actual: Vec<_> = bytes[FLOAT32_WAV_HEADER_BYTES as usize..]
                    .chunks_exact(4)
                    .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
                    .collect();
                if let Some((expected_frames, expected)) = &reference {
                    assert_eq!(
                        report.frames, *expected_frames,
                        "block={block}, devices={index}, tails={tails}"
                    );
                    assert_eq!(
                        &actual, expected,
                        "block={block}, devices={index}, tails={tails}"
                    );
                } else {
                    reference = Some((report.frames, actual));
                }
            }
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
