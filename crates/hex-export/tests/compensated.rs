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
    for (track_delay, main_delay) in [(false, false), (true, false), (false, true), (true, true)] {
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
    let refused_bounce = directory.join("latency-bounce.wav");
    let bounce = hex_export::render_arrangement_bounce_new(
        &refused_bounce,
        Arc::clone(&prepared),
        None,
        0,
        17,
        false,
        &hex_export::ExportControl::default(),
    );
    if track_delay || main_delay {
        assert!(bounce.unwrap_err().to_string().contains("zero-latency"));
        assert!(!refused_bounce.exists());
    } else {
        assert!(bounce.is_ok());
    }
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
    assert_pcm_bytes_near(&a[44..], &expected, "legacy");
    use hex_export::{ExportControl, ExportSignal, controlled_frame_count};
    let main = ExportSignal::Main;
    // Main is latency-compensated: the automatic range is the source, not startup.
    let frames = controlled_frame_count(prepared.arrangement(), 0, None, &main).unwrap();
    assert_eq!(frames, 17);
    assert_eq!(
        controlled_frame_count(prepared.arrangement(), 0, Some(17), &main).unwrap(),
        17
    );
    let track = editor.project().tracks()[0].id();
    let capture = ExportSignal::CapturedMix(vec![hex_engine::OutputCaptureId::Track(track)]);
    let capture_latency = 7 * u64::from(track_delay);
    assert_eq!(
        controlled_frame_count(prepared.arrangement(), 0, None, &capture).unwrap(),
        17 + capture_latency
    );
    assert_eq!(
        controlled_frame_count(
            prepared.arrangement(),
            0,
            None,
            &ExportSignal::Stems(vec![])
        )
        .unwrap(),
        17 + capture_latency
    );
    assert_eq!(
        controlled_frame_count(prepared.arrangement(), 0, Some(17), &capture).unwrap(),
        17
    );
    let mut capture_expected = vec![0_u8; capture_latency as usize * 4];
    capture_expected.extend_from_slice(&expected);
    let starts: &[u64] = if track_delay { &[0, 3, 19] } else { &[0, 3] };
    for &start in starts {
        let span = controlled_frame_count(prepared.arrangement(), start, None, &capture).unwrap();
        let path = directory.join(format!("capture-{start}.wav"));
        let report = hex_export::render_prepared_controlled(
            &path,
            Arc::clone(&prepared),
            start,
            span,
            capture.clone(),
            &ExportControl::default(),
        )
        .unwrap();
        assert_eq!(report.frames, 17 + capture_latency - start);
        assert_pcm_bytes_near(
            &std::fs::read(path).unwrap()[44..],
            &capture_expected[start as usize * 4..],
            "capture",
        );
    }
    let stem_path = directory.join("stems");
    let stem = hex_export::Stem {
        id: hex_engine::OutputCaptureId::Track(track),
        name: "source".into(),
    };
    let stem_span = controlled_frame_count(
        prepared.arrangement(),
        0,
        None,
        &ExportSignal::Stems(vec![stem.clone()]),
    )
    .unwrap();
    hex_export::render_stems_new(
        &stem_path,
        Arc::clone(&prepared),
        0,
        stem_span,
        &[stem],
        &ExportControl::default(),
    )
    .unwrap();
    let stem_file = stem_path.join(format!("track-{}-source.wav", track.value()));
    assert_pcm_bytes_near(
        &std::fs::read(stem_file).unwrap()[44..],
        &capture_expected,
        "stem",
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
        assert_pcm_bytes_near(&std::fs::read(path).unwrap()[44..], &expected, label);
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn automatic_return_and_stems_include_pre_main_latency_but_not_main_latency() {
    use hex_engine::OutputCaptureId;
    use hex_export::{ExportControl, ExportSignal, Stem, controlled_frame_count};

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 17,
            playback_frames: 17,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack { name: "bus".into() })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: Definition.default_state().unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: Definition.default_state().unwrap(),
        })
        .unwrap();
    let mut samples = vec![[0.0; 2]; 17];
    samples[0] = [0.25, -0.5];
    samples[16] = [-0.75, 0.125];
    let media = BTreeMap::from([(asset, Audio::new(48_000, samples.clone()).unwrap())]);
    let effects: [&dyn NativeDeviceDefinition; 1] = [&Definition];
    let effects = NativeDeviceRegistry::new(&effects).unwrap();
    let prepared = Arc::new(
        PreparedRevision::prepare_with_test_registries(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
            &effects,
            &hex_engine_instruments(),
        )
        .unwrap(),
    );
    assert_eq!(prepared.arrangement().main_latency_frames(), 14);
    let signal = ExportSignal::CapturedMix(vec![OutputCaptureId::Return(bus)]);
    let frames = controlled_frame_count(prepared.arrangement(), 0, None, &signal).unwrap();
    assert_eq!(frames, 24);
    let directory = std::env::temp_dir().join(format!(
        "hex-return-capture-{}-{}",
        std::process::id(),
        NEXT_RETURN.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("return.wav");
    hex_export::render_prepared_controlled(
        &path,
        Arc::clone(&prepared),
        0,
        frames,
        signal,
        &ExportControl::default(),
    )
    .unwrap();
    let mut expected = vec![0_u8; 7 * 4];
    expected.extend(
        samples
            .iter()
            .flatten()
            .flat_map(|s| ((s * 32767.0_f32).round() as i16).to_le_bytes()),
    );
    assert_pcm_bytes_near(&std::fs::read(path).unwrap()[44..], &expected, "return");

    let stems = [
        Stem {
            id: OutputCaptureId::Track(track),
            name: "source".into(),
        },
        Stem {
            id: OutputCaptureId::Return(bus),
            name: "bus".into(),
        },
    ];
    let batch = directory.join("stems");
    let frames = controlled_frame_count(
        prepared.arrangement(),
        0,
        None,
        &ExportSignal::Stems(stems.to_vec()),
    )
    .unwrap();
    assert_eq!(frames, 24);
    hex_export::render_stems_new(
        &batch,
        prepared,
        0,
        frames,
        &stems,
        &ExportControl::default(),
    )
    .unwrap();
    assert_pcm_bytes_near(
        &std::fs::read(batch.join(format!("track-{}-source.wav", track.value()))).unwrap()[44..],
        &expected,
        "track stem",
    );
    assert_pcm_bytes_near(
        &std::fs::read(batch.join(format!("return-{}-bus.wav", bus.value()))).unwrap()[44..],
        &expected,
        "return stem",
    );
    std::fs::remove_dir_all(directory).unwrap();
}

/// Time-selection Main export with track and Main latency (7 + 7 frames): file
/// frame 0 is the selection start and the length is exactly the requested span.
/// Spans below and above the renderer's 256-frame block, and starts that need
/// stateful preroll, cover the discard crossing block boundaries.
#[test]
fn main_selection_export_is_latency_compensated_to_the_selection_start() {
    use hex_export::{ExportControl, ExportSignal, controlled_frame_count};
    const SOURCE: u64 = 700;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "ramp".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48_000,
            source_frames: SOURCE,
            playback_frames: SOURCE,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: Definition.default_state().unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: Definition.default_state().unwrap(),
        })
        .unwrap();
    // Every frame is distinct and audible, so any offset or truncation shows.
    let samples: Vec<[f32; 2]> = (0..SOURCE)
        .map(|i| {
            let value = ((i % 97) + 1) as f32 / 256.0;
            [value, -value]
        })
        .collect();
    let pcm: Vec<u8> = samples
        .iter()
        .flatten()
        .flat_map(|sample| ((sample * 32767.0_f32).round() as i16).to_le_bytes())
        .collect();
    let media = BTreeMap::from([(asset, Audio::new(48_000, samples).unwrap())]);
    let effects: [&dyn NativeDeviceDefinition; 1] = [&Definition];
    let effects = NativeDeviceRegistry::new(&effects).unwrap();
    let prepared = Arc::new(
        PreparedRevision::prepare_with_test_registries(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
            &effects,
            &hex_engine_instruments(),
        )
        .unwrap(),
    );
    assert_eq!(prepared.arrangement().main_latency_frames(), 14);
    let directory = std::env::temp_dir().join(format!(
        "hex-selection-latency-{}-{}",
        std::process::id(),
        NEXT_RETURN.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let main = ExportSignal::Main;
    for (start, requested) in [
        (0, Some(17)),
        (300, Some(17)),
        (300, Some(300)),
        (250, Some(400)),
        (300, None),
    ] {
        let span = controlled_frame_count(prepared.arrangement(), start, requested, &main).unwrap();
        assert_eq!(span, requested.unwrap_or(SOURCE - start), "{start}");
        let path = directory.join(format!("selection-{start}-{span}.wav"));
        let report = hex_export::render_prepared_controlled(
            &path,
            Arc::clone(&prepared),
            start,
            span,
            main.clone(),
            &ExportControl::default(),
        )
        .unwrap();
        assert_eq!(report.frames, span);
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len() as u64, 44 + span * 4);
        let range = start as usize * 4..(start + span) as usize * 4;
        assert_pcm_bytes_near(
            &bytes[44..],
            &pcm[range],
            &format!("start {start}, span {span}"),
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

static NEXT_RETURN: AtomicU64 = AtomicU64::new(0);
// Only the effect fixture is used; the empty instrument registry is truthful.
fn hex_engine_instruments() -> hex_lattice::NativeInstrumentRegistry<'static> {
    hex_lattice::NativeInstrumentRegistry::new(&[]).unwrap()
}

#[test]
fn automatic_ranges_include_finite_serial_and_routed_return_tails() {
    use hex_engine::OutputCaptureId;
    use hex_export::{ExportControl, ExportSignal, Stem, controlled_frame_count};
    use hex_lattice::conformance::owned_delay;

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 24).unwrap();
    let mut tracks = Vec::new();
    let mut media = BTreeMap::new();
    for (name, value) in [("serial", 0.25_f32), ("send", 0.125)] {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: name.into(),
                path: format!("{name}.wav").into(),
                source_sample_rate: 48_000,
                source_frames: 1,
                playback_frames: 1,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("fixture track")
        };
        tracks.push(track);
        media.insert(asset, Audio::new(48_000, vec![[value, -value]]).unwrap());
    }
    let delay = || owned_delay::state(4).unwrap();
    for track in [tracks[0], tracks[0], tracks[1]] {
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: delay(),
            })
            .unwrap();
    }
    let Created::ReturnTrack(first) = editor
        .execute(Command::AddReturnTrack {
            name: "first".into(),
        })
        .unwrap()
    else {
        panic!("first return")
    };
    let Created::ReturnTrack(second) = editor
        .execute(Command::AddReturnTrack {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("second return")
    };
    editor
        .execute(Command::SetReturnSend {
            track: tracks[1],
            return_track: first,
            gain: 1.0,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnFeed {
            source: first,
            destination: second,
            gain: 1.0,
        })
        .unwrap();
    for bus in [first, second] {
        editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: delay(),
            })
            .unwrap();
    }
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: delay(),
        })
        .unwrap();
    let effects: [&dyn NativeDeviceDefinition; 1] = [&owned_delay::DEFINITION];
    let registry = NativeDeviceRegistry::new(&effects).unwrap();
    let prepared = Arc::new(
        PreparedRevision::prepare_with_test_registries(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
            &registry,
            &hex_engine_instruments(),
        )
        .unwrap(),
    );
    let serial = OutputCaptureId::Track(tracks[0]);
    let send = OutputCaptureId::Track(tracks[1]);
    let first = OutputCaptureId::Return(first);
    let second = OutputCaptureId::Return(second);
    let main = ExportSignal::Main;
    assert_eq!(
        controlled_frame_count(prepared.arrangement(), 0, None, &main).unwrap(),
        17
    );
    for (id, expected) in [(serial, 9), (send, 5), (first, 9), (second, 13)] {
        assert_eq!(
            controlled_frame_count(
                prepared.arrangement(),
                0,
                None,
                &ExportSignal::CapturedMix(vec![id])
            )
            .unwrap(),
            expected
        );
    }
    let selected = [serial, send, first, second];
    let stems: Vec<_> = selected
        .iter()
        .enumerate()
        .map(|(index, &id)| Stem {
            id,
            name: format!("part{index}"),
        })
        .collect();
    assert_eq!(
        controlled_frame_count(
            prepared.arrangement(),
            0,
            None,
            &ExportSignal::Stems(stems.clone())
        )
        .unwrap(),
        13
    );
    let directory = std::env::temp_dir().join(format!(
        "hex-finite-tail-{}-{}",
        std::process::id(),
        NEXT_RETURN.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let main_path = directory.join("main.wav");
    hex_export::render_prepared_controlled(
        &main_path,
        Arc::clone(&prepared),
        0,
        17,
        main,
        &ExportControl::default(),
    )
    .unwrap();
    let bytes = std::fs::read(main_path).unwrap();
    assert_ne!(
        &bytes[44 + 16 * 4..44 + 17 * 4],
        &[0; 4],
        "final Main tail sample"
    );
    let batch = directory.join("stems");
    hex_export::render_stems_new(
        &batch,
        Arc::clone(&prepared),
        0,
        13,
        &stems,
        &ExportControl::default(),
    )
    .unwrap();
    for (index, id, last) in [(0, serial, 8), (1, send, 4), (2, first, 8), (3, second, 12)] {
        let (kind, value) = match id {
            OutputCaptureId::Track(id) => ("track", id.value()),
            OutputCaptureId::Return(id) => ("return", id.value()),
        };
        let bytes = std::fs::read(batch.join(format!("{kind}-{value}-part{index}.wav"))).unwrap();
        assert_ne!(
            &bytes[44 + last * 4..44 + (last + 1) * 4],
            &[0; 4],
            "last stem sample {index}"
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

/// PCM16 export applies ±1 LSB TPDF dither, so each decoded sample lands
/// within one LSB of the plain-rounding oracle bytes.
fn assert_pcm_bytes_near(actual: &[u8], expected: &[u8], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}");
    for (index, (actual, expected)) in actual
        .chunks_exact(2)
        .zip(expected.chunks_exact(2))
        .enumerate()
    {
        let actual = i16::from_le_bytes([actual[0], actual[1]]);
        let expected = i16::from_le_bytes([expected[0], expected[1]]);
        assert!(
            (i32::from(actual) - i32::from(expected)).abs() <= 1,
            "{context} sample {index}: {actual} not within one lsb of {expected}"
        );
    }
}
