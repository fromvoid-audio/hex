#![cfg(feature = "instrument-test-support")]
//! Export's source range excludes causal startup while retaining its final sample.
use hex_engine::{Audio, PreparedRevision, compile};
#[path = "../../hex-engine/examples/support/latency_fixture.rs"]
mod latency_fixture;
use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{Command, Created, Editor, Project};
use latency_fixture::Definition;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
#[test]
fn compensated_export_skips_startup_and_publishes_exact_first_and_final_source_frames() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48000,
            source_frames: 17,
            playback_frames: 17,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: Definition.default_state().unwrap(),
        })
        .unwrap();
    let mut samples = vec![[0.0; 2]; 17];
    samples[0] = [0.25, -0.5];
    samples[16] = [-0.75, 0.125];
    let media = BTreeMap::from([(asset, Audio::new(48000, samples.clone()).unwrap())]);
    let effects: [&dyn NativeDeviceDefinition; 1] = [&Definition];
    let effects = NativeDeviceRegistry::new(&effects).unwrap();
    let instruments = hex_engine_instruments();
    let prepared = Arc::new(
        PreparedRevision::prepare_with_test_registries(
            editor.project(),
            &media,
            hex_engine::SharedSineTable::new_on_control(),
            None,
            &effects,
            &instruments,
        )
        .unwrap(),
    );
    let compiled = compile::project_with_registry(editor.project(), &media, &effects).unwrap();
    assert_eq!(compiled.main_latency_frames(), 7);
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "hex-compensated-export-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let legacy = directory.join("legacy.wav");
    let shared = directory.join("prepared.wav");
    let a = hex_export::render_new(&legacy, &compiled, 0, 17).unwrap();
    let b = hex_export::render_prepared_new(&shared, prepared, 0, 17).unwrap();
    assert_eq!((a.frames, b.frames), (17, 17));
    let a = std::fs::read(&legacy).unwrap();
    let b = std::fs::read(&shared).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len(), 44 + 17 * 4);
    let expected: Vec<_> = samples
        .iter()
        .flatten()
        .flat_map(|sample| ((sample * 32767.0_f32).round() as i16).to_le_bytes())
        .collect();
    assert_eq!(&a[44..], expected);
    std::fs::remove_dir_all(directory).unwrap();
}
// Only the effect fixture is used; the empty instrument registry is truthful.
fn hex_engine_instruments() -> hex_lattice::NativeInstrumentRegistry<'static> {
    hex_lattice::NativeInstrumentRegistry::new(&[]).unwrap()
}
