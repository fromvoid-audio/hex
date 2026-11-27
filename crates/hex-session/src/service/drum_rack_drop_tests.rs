//! End-to-end Drum Rack pad drop acceptance tests (#3476):
//! sample + native instrument pad drops, shared retained source semantics,
//! audible distinct pads, Undo/Redo, save/reopen, stale target, project
//! replacement, unsupported/failed import, and cancellation.

use super::*;
use hex_engine::{
    AudioStorageLedgerReservation, Frame, NativeDeviceClass, PreparedRevision, SessionPlayback,
    SessionPlaybackCommand, SineTable,
    native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets},
};
use hex_lattice_contract::DeviceTypeId;
use hex_project::{BeatTime, Command, Created};
use std::{
    collections::BTreeMap,
    num::NonZeroU64,
    sync::atomic::{AtomicBool, AtomicU64},
};

const RATE: u32 = 48_000;
const SUBTRACTIVE_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.subtractive");
const SINE_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.sine");

struct TestDir(PathBuf);

impl TestDir {
    fn new(prefix: &str) -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(1);
        let id = SERIAL.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("hex-{prefix}-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn reservation() -> crate::ResidentNativeReservation {
    crate::ResidentNativeReservation {
        storage: AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 128,
            root_rows: 64,
            max_retained_bytes: 32 * 1024 * 1024,
            max_construction_peak_bytes: 32 * 1024 * 1024,
        },
        max_scan_frames: 2_000_000,
    }
}

fn recv(service: &Service) -> Event {
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .expect("worker event")
}

fn write_test_wav(path: &Path, sample_rate: u32, frames: usize, amp: f32) {
    let channels: u16 = 2;
    let bits_per_sample: u16 = 16;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let data_bytes = u32::try_from(frames * usize::from(block_align)).unwrap();
    let riff_size = 36 + data_bytes;

    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for i in 0..frames {
        let phase = 2.0 * std::f32::consts::PI * 220.0 * (i as f32) / (sample_rate as f32);
        let sample = (phase.sin() * amp).clamp(-1.0, 1.0);
        let pcm16 = (sample * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&pcm16.to_le_bytes());
        out.extend_from_slice(&pcm16.to_le_bytes());
    }
    std::fs::write(path, out).expect("write test wav");
}

fn render_project_peak(
    project: &Project,
    wav_files: &[(hex_project::AssetId, &Path)],
) -> Vec<Frame> {
    let cancel = AtomicBool::new(false);
    let mut store = ResidentDeviceAssets::default();
    let limits = reservation();
    for &(asset_id, path) in wav_files {
        let decoded = decode_wav_file(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))
            .expect("decode wav");
        let (_, source) = decoded
            .into_native_source_on_control(limits.max_scan_frames, &cancel)
            .expect("native source")
            .into_parts();
        store
            .insert_checked_on_control(
                (asset_id, source),
                0,
                limits.storage,
                limits.max_scan_frames,
                &cancel,
                |_| Ok::<_, hex_engine::native_device_resources::ResourcePreparationError>(()),
            )
            .expect("store insert");
    }
    let resources = PreparedDeviceResources::prepare_on_control(
        &store,
        project,
        limits.storage.max_retained_bytes,
        &cancel,
    )
    .expect("prepared device resources");
    let table = Arc::new(SineTable::new());
    let transforms = hex_engine::audio_transform::AudioTransformPreparation::default();
    let revision = Arc::new(
        PreparedRevision::prepare_with_device_resources(
            project,
            &BTreeMap::new(),
            table,
            None,
            &transforms,
            &resources,
            limits.max_scan_frames,
            &cancel,
        )
        .expect("prepared revision"),
    );
    let mut playback =
        SessionPlayback::new_with_device_resources(revision, &resources).expect("session playback");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut out = vec![[0.0_f32; 2]; 2048];
    playback.render(&mut out).expect("render");
    out
}

fn rms(frames: &[Frame]) -> f32 {
    if frames.is_empty() {
        return 0.0;
    }
    let sum: f32 = frames.iter().map(|[l, r]| 0.5 * (l * l + r * r)).sum();
    (sum / frames.len() as f32).sqrt()
}

fn max_diff(a: &[Frame], b: &[Frame]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|([al, ar], [bl, br])| (al - bl).abs().max((ar - br).abs()))
        .fold(0.0_f32, f32::max)
}

#[test]
fn sample_and_instrument_pad_drops_share_source_render_distinct_audio_undo_redo_and_reopen() {
    let dir = TestDir::new("drum-rack-e2e");
    let kick_path = dir.path().join("kick_one.wav");
    write_test_wav(&kick_path, RATE, 1024, 0.75);

    let service = Service::with_native_reservation(RATE, 32, 8_000_000, reservation())
        .expect("service with native reservation");
    let _init = recv(&service);

    // Create one shared retained Drum Rack source bound to two MIDI contributor tracks.
    let rack_state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_engine::DRUM_RACK_TYPE_ID,
    )
    .expect("default drum rack");
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "shared drums".into(),
            devices: vec![rack_state],
            instrument_index: 0,
        }))
        .unwrap();
    let ev = recv(&service);
    assert!(ev.error.is_none(), "{:?}", ev.error);
    let Some(Created::NativeSource(source_id)) = ev.created else {
        panic!("expected native source");
    };
    let rack_device = ev
        .state
        .project
        .native_source(source_id)
        .unwrap()
        .instrument()
        .id();

    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "kick midi".into(),
        }))
        .unwrap();
    let ev_a = recv(&service);
    let Some(Created::Track(track_a)) = ev_a.created else {
        panic!("track a");
    };
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "synth midi".into(),
        }))
        .unwrap();
    let ev_b = recv(&service);
    let Some(Created::Track(track_b)) = ev_b.created else {
        panic!("track b");
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: track_a,
            source: Some(source_id),
        }))
        .unwrap();
    let _ = recv(&service);
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: track_b,
            source: Some(source_id),
        }))
        .unwrap();
    let bound = recv(&service);
    assert!(bound.error.is_none(), "{:?}", bound.error);
    assert!(
        bound
            .state
            .project
            .track(track_a)
            .unwrap()
            .receives_source_audio()
    );
    assert!(
        !bound
            .state
            .project
            .track(track_b)
            .unwrap()
            .receives_source_audio()
    );

    // 1. Drop sample onto pad 1 (receive note 36 = C1).
    let pad1 = NonZeroU64::new(1).unwrap();
    service
        .submit(Request::DropDrumRackPad {
            request_id: 1,
            project_generation: 0,
            revision: bound.state.revision,
            device: rack_device,
            pad_id: pad1,
            payload: DrumRackPadDropPayload::Sample {
                path: kick_path.clone(),
            },
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let after_sample = recv(&service);
    assert!(after_sample.error.is_none(), "{:?}", after_sample.error);
    let Some(Created::Asset(sample_asset)) = after_sample.created else {
        panic!("expected imported sample asset");
    };

    // Shared source semantics: both tracks see the updated Drum Rack state.
    assert_eq!(
        after_sample
            .state
            .project
            .track(track_a)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        after_sample
            .state
            .project
            .track(track_b)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
    );

    // 2. Drop built-in leaf native instrument (Subtractive) onto pad 2 (receive note 37 = C#1).
    let pad2 = NonZeroU64::new(2).unwrap();
    service
        .submit(Request::DropDrumRackPad {
            request_id: 2,
            project_generation: 0,
            revision: after_sample.state.revision,
            device: rack_device,
            pad_id: pad2,
            payload: DrumRackPadDropPayload::Instrument {
                type_id: SUBTRACTIVE_TYPE_ID,
            },
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let after_both = recv(&service);
    assert!(after_both.error.is_none(), "{:?}", after_both.error);
    let views = hex_engine::drum_rack_pad_views_on_control(
        after_both
            .state
            .project
            .native_source(source_id)
            .unwrap()
            .instrument()
            .state(),
    )
    .unwrap();
    assert!(views[0].occupied);
    assert_eq!(views[0].name, "kick_one");
    assert!(views[1].occupied);
    assert_eq!(views[1].child_type_id, Some(SUBTRACTIVE_TYPE_ID));

    // 3. Verify audible distinct pads across the two shared contributor tracks.
    let one_beat = BeatTime::from_quarters(1).unwrap();
    let mut only_pad1_editor =
        hex_project::Editor::new((*after_both.state.project).clone(), 4).unwrap();
    only_pad1_editor
        .execute(Command::PlaceMidiClipWithNotes {
            track: track_a,
            start: BeatTime::ZERO,
            length: one_beat,
            notes: vec![hex_project::MidiNoteInsert {
                pitch: 36,
                velocity: 110,
                start: BeatTime::ZERO,
                duration: one_beat,
                channel: 0,
                active: true,
            }],
        })
        .unwrap();
    let pad1_audio = render_project_peak(only_pad1_editor.project(), &[(sample_asset, &kick_path)]);
    assert!(
        rms(&pad1_audio) > 0.05,
        "pad 1 sample must render audible pcm, rms={}",
        rms(&pad1_audio)
    );

    let mut only_pad2_editor =
        hex_project::Editor::new((*after_both.state.project).clone(), 4).unwrap();
    only_pad2_editor
        .execute(Command::PlaceMidiClipWithNotes {
            track: track_b,
            start: BeatTime::ZERO,
            length: one_beat,
            notes: vec![hex_project::MidiNoteInsert {
                pitch: 37,
                velocity: 110,
                start: BeatTime::ZERO,
                duration: one_beat,
                channel: 0,
                active: true,
            }],
        })
        .unwrap();
    let pad2_audio = render_project_peak(only_pad2_editor.project(), &[(sample_asset, &kick_path)]);
    assert!(
        rms(&pad2_audio) > 0.01,
        "pad 2 instrument must render audible pcm from shared contributor track, rms={}",
        rms(&pad2_audio)
    );
    assert!(
        max_diff(&pad1_audio, &pad2_audio) > 0.05,
        "pad 1 sample and pad 2 instrument must produce distinct audio"
    );

    // 4. Undo / Redo restores each pad cleanly in single history steps.
    service.submit(Request::Undo).unwrap();
    let undone_inst = recv(&service);
    assert!(undone_inst.error.is_none(), "{:?}", undone_inst.error);
    let views = hex_engine::drum_rack_pad_views_on_control(
        undone_inst
            .state
            .project
            .native_source(source_id)
            .unwrap()
            .instrument()
            .state(),
    )
    .unwrap();
    assert!(views[0].occupied, "undo pad 2 must keep pad 1 sample");
    assert!(!views[1].occupied, "undo must clear pad 2 instrument");

    service.submit(Request::Undo).unwrap();
    let undone_sample = recv(&service);
    assert!(undone_sample.error.is_none(), "{:?}", undone_sample.error);
    assert!(
        undone_sample.state.project.asset(sample_asset).is_none(),
        "undoing sample pad drop must remove its imported asset"
    );

    service.submit(Request::Redo).unwrap();
    let _ = recv(&service);
    service.submit(Request::Redo).unwrap();
    let redone_both = recv(&service);
    assert!(redone_both.error.is_none(), "{:?}", redone_both.error);
    assert!(redone_both.state.project.asset(sample_asset).is_some());

    // 5. Save and reopen in a fresh Service with resident reservation.
    let project_path = dir.path().join("drum_rack_pads.hexproj");
    service
        .submit(Request::SaveNew(project_path.clone()))
        .unwrap();
    let saved = recv(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);

    let reopened_service = Service::with_native_reservation(RATE, 32, 8_000_000, reservation())
        .expect("second service");
    let _ = recv(&reopened_service);
    reopened_service
        .submit(Request::Open(project_path))
        .unwrap();
    let reopened = recv(&reopened_service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert!(reopened.project_replaced);
    let reopened_source = reopened
        .state
        .project
        .native_source(source_id)
        .expect("reopened shared drum rack source");
    let reopened_views =
        hex_engine::drum_rack_pad_views_on_control(reopened_source.instrument().state()).unwrap();
    assert!(reopened_views[0].occupied);
    assert_eq!(reopened_views[0].name, "kick_one");
    assert!(reopened_views[1].occupied);
    assert_eq!(reopened_views[1].child_type_id, Some(SUBTRACTIVE_TYPE_ID));
    assert!(
        reopened
            .state
            .project
            .asset(sample_asset)
            .and_then(|asset| asset.original_source())
            .is_some(),
        "saved drum rack sample asset must retain verified original_source facts"
    );
}

#[test]
fn stale_target_project_replacement_unsupported_failed_import_and_cancel_leave_pads_intact() {
    let dir = TestDir::new("drum-rack-refusal");
    let good_wav = dir.path().join("snare.wav");
    write_test_wav(&good_wav, RATE, 512, 0.5);
    let corrupt_wav = dir.path().join("corrupt.wav");
    std::fs::write(&corrupt_wav, b"not a valid wav file").unwrap();

    let service =
        Service::with_native_reservation(RATE, 32, 8_000_000, reservation()).expect("service");
    let _ = recv(&service);

    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "drums".into(),
        }))
        .unwrap();
    let ev_track = recv(&service);
    let Some(Created::Track(track)) = ev_track.created else {
        panic!("track");
    };
    let rack_state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_engine::DRUM_RACK_TYPE_ID,
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertInstrument {
            track,
            state: rack_state,
        }))
        .unwrap();
    let ev_rack = recv(&service);
    let Some(Created::Device(rack_device)) = ev_rack.created else {
        panic!("rack");
    };

    // Seed pad 1 with a valid sample so we verify every refusal leaves it untouched.
    let pad1 = NonZeroU64::new(1).unwrap();
    let pad2 = NonZeroU64::new(2).unwrap();
    service
        .submit(Request::DropDrumRackPad {
            request_id: 1,
            project_generation: 0,
            revision: ev_rack.state.revision,
            device: rack_device,
            pad_id: pad1,
            payload: DrumRackPadDropPayload::Sample {
                path: good_wav.clone(),
            },
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let baseline = recv(&service);
    assert!(baseline.error.is_none(), "{:?}", baseline.error);
    let baseline_project = Arc::clone(&baseline.state.project);
    let baseline_rev = baseline.state.revision;

    // 1. Stale revision -> refused, prior pads and assets intact.
    service
        .submit(Request::DropDrumRackPad {
            request_id: 2,
            project_generation: 0,
            revision: baseline_rev.saturating_sub(1),
            device: rack_device,
            pad_id: pad2,
            payload: DrumRackPadDropPayload::Instrument {
                type_id: SINE_TYPE_ID,
            },
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let stale_rev = recv(&service);
    assert!(stale_rev.error.is_some());
    assert_eq!(stale_rev.state.project.as_ref(), baseline_project.as_ref());

    // 2. Stale project generation -> refused.
    service
        .submit(Request::DropDrumRackPad {
            request_id: 3,
            project_generation: 99,
            revision: baseline_rev,
            device: rack_device,
            pad_id: pad2,
            payload: DrumRackPadDropPayload::Instrument {
                type_id: SINE_TYPE_ID,
            },
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let stale_gen = recv(&service);
    assert!(stale_gen.error.is_some());
    assert_eq!(stale_gen.state.project.as_ref(), baseline_project.as_ref());

    // 3. Unsupported nested Drum Rack / unknown instrument -> refused.
    for bad_type in [
        hex_engine::DRUM_RACK_TYPE_ID,
        DeviceTypeId::from_static("hex.instrument.nonexistent"),
    ] {
        service
            .submit(Request::DropDrumRackPad {
                request_id: 4,
                project_generation: 0,
                revision: baseline_rev,
                device: rack_device,
                pad_id: pad2,
                payload: DrumRackPadDropPayload::Instrument { type_id: bad_type },
                cancel: Arc::new(AtomicBool::new(false)),
            })
            .unwrap();
        let refused = recv(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project.as_ref(), baseline_project.as_ref());
    }

    // 4. Failed / corrupt sample import -> refused without mutating pads or leaving orphaned assets.
    service
        .submit(Request::DropDrumRackPad {
            request_id: 5,
            project_generation: 0,
            revision: baseline_rev,
            device: rack_device,
            pad_id: pad2,
            payload: DrumRackPadDropPayload::Sample { path: corrupt_wav },
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let corrupt = recv(&service);
    assert!(corrupt.error.is_some());
    assert_eq!(corrupt.state.project.as_ref(), baseline_project.as_ref());

    // 5. Cancelled drop -> refused without mutating pads or assets.
    service
        .submit(Request::DropDrumRackPad {
            request_id: 6,
            project_generation: 0,
            revision: baseline_rev,
            device: rack_device,
            pad_id: pad2,
            payload: DrumRackPadDropPayload::Sample { path: good_wav },
            cancel: Arc::new(AtomicBool::new(true)),
        })
        .unwrap();
    let cancelled = recv(&service);
    assert!(cancelled.error.is_some());
    assert_eq!(cancelled.state.project.as_ref(), baseline_project.as_ref());

    // 6. Project replacement invalidates old generation/device drops cleanly.
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    let replaced = recv(&service);
    assert!(replaced.project_replaced);
    service
        .submit(Request::DropDrumRackPad {
            request_id: 7,
            project_generation: 0,
            revision: baseline_rev,
            device: rack_device,
            pad_id: pad2,
            payload: DrumRackPadDropPayload::Instrument {
                type_id: SINE_TYPE_ID,
            },
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let post_replace = recv(&service);
    assert!(post_replace.error.is_some());
    assert_eq!(post_replace.state.project.tracks().len(), 0);
}
