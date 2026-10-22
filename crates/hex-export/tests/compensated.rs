#![cfg(feature = "instrument-test-support")]
//! Export's source range excludes causal startup while retaining its final sample.
use hex_engine::{Audio, PreparedRevision, SineTable, compile};
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
    for (track_delay, main_delay) in [(true, false), (false, true), (true, true)] {
        assert_default_export_final_frame(track_delay, main_delay);
    }
}
fn assert_default_export_final_frame(track_delay: bool, main_delay: bool) {
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
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    if track_delay {
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: Definition.default_state().unwrap(),
            })
            .unwrap();
    }
    if main_delay {
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: Definition.default_state().unwrap(),
            })
            .unwrap();
    }
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
            Arc::new(SineTable::new()),
            None,
            &effects,
            &instruments,
        )
        .unwrap(),
    );
    let compiled = compile::project_with_registry(editor.project(), &media, &effects).unwrap();
    assert_eq!(
        compiled.main_latency_frames(),
        7 * (u32::from(track_delay) + u32::from(main_delay))
    );
    let default_end = compiled.audible_end_frame().unwrap();
    assert_eq!(
        default_end, 17,
        "source range excludes discarded causal startup"
    );
    assert_eq!(
        prepared.arrangement().audible_end_frame(),
        Some(default_end)
    );
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "hex-compensated-export-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let legacy = directory.join("legacy.wav");
    let shared = directory.join("prepared.wav");
    let a = hex_export::render_new(&legacy, &compiled, 0, default_end).unwrap();
    let b =
        hex_export::render_prepared_new(&shared, Arc::clone(&prepared), 0, default_end).unwrap();
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
    use hex_export::{ExportControl, ExportSignal, controlled_frame_count};
    let main = ExportSignal::Main;
    let frames = controlled_frame_count(prepared.arrangement(), 0, None, &main).unwrap();
    assert_eq!(frames, 17 + u64::from(compiled.main_latency_frames()));
    assert_eq!(
        controlled_frame_count(prepared.arrangement(), 0, Some(17), &main).unwrap(),
        17
    );
    let track = editor.project().tracks()[0].id();
    let capture = ExportSignal::CapturedMix(vec![hex_engine::OutputCaptureId::Track(track)]);
    assert_eq!(
        controlled_frame_count(prepared.arrangement(), 0, None, &capture).unwrap(),
        17
    );
    assert_eq!(
        controlled_frame_count(
            prepared.arrangement(),
            0,
            None,
            &ExportSignal::Stems(vec![])
        )
        .unwrap(),
        17
    );
    for (label, span) in [("automatic", frames), ("explicit", 17)] {
        let path = directory.join(format!("{label}.wav"));
        let report = hex_export::render_prepared_controlled(
            &path,
            Arc::clone(&prepared),
            0,
            span,
            ExportSignal::Main,
            &ExportControl::default(),
        )
        .unwrap();
        assert_eq!(report.frames, span);
        let bytes = std::fs::read(path).unwrap();
        let latency = compiled.main_latency_frames() as usize;
        let mut causal = vec![0; latency * 4];
        causal.extend_from_slice(&expected);
        assert_eq!(&bytes[44..], &causal[..span as usize * 4]);
    }
    std::fs::remove_dir_all(directory).unwrap();
}
// Only the effect fixture is used; the empty instrument registry is truthful.
fn hex_engine_instruments() -> hex_lattice::NativeInstrumentRegistry<'static> {
    hex_lattice::NativeInstrumentRegistry::new(&[]).unwrap()
}
