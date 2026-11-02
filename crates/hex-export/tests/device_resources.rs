//! Ordinary resource export uses real registered DSP and an independent scalar mix oracle.
use hex_engine::native_device_resources::{
    PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
};
use hex_engine::{
    Audio, AudioStorageLedgerReservation, OutputCaptureId, PreparedRevision, SessionPlayback,
    SineTable,
};
use hex_export::{ExportControl, ExportSignal, Stem};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceTypeId, ParameterId, ParameterValue,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const RATE: u32 = 8000;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-resource-export-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Fixture {
    revision: Arc<PreparedRevision>,
    resources: PreparedDeviceResources,
    sampler: hex_project::TrackId,
    bus: hex_project::ReturnId,
}
fn utility(gain: f32) -> hex_lattice_contract::NativeDeviceState {
    hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility")).unwrap(),
        ParameterId::from_static(1),
        ParameterValue::Float(gain),
    )
    .unwrap()
}
fn fixture(midi_loop: bool) -> Fixture {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "original.wav".into(),
            sample_rate: RATE,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let mut sources = ResidentDeviceAssets::default();
    sources
        .insert_on_control(
            (
                asset,
                hex_engine::OwnedAudioSource::try_resident(RATE, vec![[0.25, -0.125]; 1024])
                    .ok()
                    .unwrap(),
            ),
            0,
            AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
            1024,
            &AtomicBool::new(false),
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    let (mut state, slot) =
        hex_engine::native_device_resources::sampler_import_template_on_control(1024).unwrap();
    state = hex_engine::patch_sampler_parameter_on_control(
        hex_engine::SamplerParameter::ReleaseMs,
        &state,
        4.0,
    )
    .unwrap();
    state = state
        .with_asset_references(vec![DeviceAssetReference {
            slot,
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    let Created::Track(sampler) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "sampler".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state,
        })
        .unwrap()
    else {
        panic!("sampler")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: sampler,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(256, 4000).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    if midi_loop {
        editor
            .execute(Command::SetMidiClipSource {
                clip,
                source_length: BeatTime::ONE,
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: BeatTime::ONE,
                looping: true,
                length: BeatTime::ONE,
            })
            .unwrap();
    }
    editor
        .execute(Command::Gain {
            track: sampler,
            gain: 0.5,
        })
        .unwrap();
    let Created::Imported {
        track: audio,
        asset: clip_asset,
        ..
    } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "audio".into(),
            path: "clip.wav".into(),
            source_sample_rate: RATE,
            source_frames: 512,
            playback_frames: 512,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    editor
        .execute(Command::Gain {
            track: audio,
            gain: 0.5,
        })
        .unwrap();
    let Created::Track(sine) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "sine".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("sine")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: sine,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(128, 4000).unwrap(),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    editor
        .execute(Command::Gain {
            track: sine,
            gain: 0.25,
        })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "send".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    for (track, gain) in [(sampler, 0.5), (audio, 0.25)] {
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain,
            })
            .unwrap();
    }
    editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: utility(0.5),
        })
        .unwrap();
    editor
        .execute(Command::ReturnGain {
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: utility(0.75),
        })
        .unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(&sources, editor.project(), 1_000_000, &cancel)
            .unwrap();
    let media = BTreeMap::from([(
        clip_asset,
        Audio::new(RATE, vec![[0.125, 0.0625]; 512]).unwrap(),
    )]);
    let work = resources
        .compiler_scan_work_on_control(editor.project())
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_device_resources(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            work,
            &cancel,
        )
        .unwrap(),
    );
    Fixture {
        revision,
        resources,
        sampler,
        bus,
    }
}
type Pcm = Vec<[f32; 2]>;
fn oracle(frames: usize) -> (Pcm, Pcm, Pcm) {
    let table = SineTable::new();
    let voice = table.prepare_voice(RATE, 69, 127).unwrap();
    let mut main = Vec::new();
    let mut sampler = Vec::new();
    let mut bus = Vec::new();
    for frame in 0..frames {
        let env = if frame < 256 {
            1.0
        } else if frame < 288 {
            1.0 - (frame - 256) as f32 / 32.0
        } else {
            0.0
        };
        let sampled = [0.125 * env, -0.0625 * env];
        let audio = if frame < 512 {
            [0.0625, 0.03125]
        } else {
            [0.0; 2]
        };
        let sine = table.sample(voice, frame as u64, 128) * 0.25;
        let returned = std::array::from_fn(|channel| {
            (sampled[channel] * 0.5 + audio[channel] * 0.25) * 0.5 * 0.5
        });
        let mixed = std::array::from_fn(|channel| {
            (sampled[channel] + audio[channel] + sine + returned[channel]) * 0.75 * 0.5
        });
        sampler.push(sampled);
        bus.push(returned);
        main.push(mixed);
    }
    (main, sampler, bus)
}
fn pcm16(path: &Path) -> Vec<[i16; 2]> {
    fs::read(path).unwrap()[44..]
        .chunks_exact(4)
        .map(|frame| {
            [
                i16::from_le_bytes([frame[0], frame[1]]),
                i16::from_le_bytes([frame[2], frame[3]]),
            ]
        })
        .collect()
}
/// PCM16 export applies ±1 LSB TPDF dither: compare within one LSB of rounding.
fn assert_near(actual: Vec<[i16; 2]>, expected: &[[f32; 2]]) {
    assert_eq!(actual.len(), expected.len());
    for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for (actual, expected) in actual.iter().zip(expected) {
            let rounded = (expected.clamp(-1.0, 1.0) * 32767.0).round() as i32;
            assert!(
                (i32::from(*actual) - rounded).abs() <= 1,
                "frame {frame}: {actual} not within one lsb of {rounded}"
            );
        }
    }
}

#[test]
fn registered_resources_export_mix_range_captures_stems_and_float_bounce() {
    // Same control-stack provision as the existing hex-export worker, never a callback stack.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let directory = Directory::new();
            let f = fixture(false);
            let (expected, sampled, returned) = oracle(640);
            let full = directory.0.join("full.wav");
            let control = ExportControl::default();
            hex_export::render_prepared_controlled_with_device_resources(
                &full,
                Arc::clone(&f.revision),
                &f.resources,
                0,
                640,
                ExportSignal::Main,
                &control,
            )
            .unwrap();
            assert_near(pcm16(&full), &expected);
            let range = directory.0.join("range.wav");
            let control = ExportControl::default();
            hex_export::render_prepared_controlled_with_device_resources(
                &range,
                Arc::clone(&f.revision),
                &f.resources,
                129,
                320,
                ExportSignal::Main,
                &control,
            )
            .unwrap();
            assert_eq!(pcm16(&range), pcm16(&full)[129..449]);
            assert_eq!(
                (
                    control.progress().processed_frames,
                    control.progress().total_frames
                ),
                (449, 449)
            );
            let default = directory.0.join("default.wav");
            hex_export::render_prepared_new_with_device_resources(
                &default,
                Arc::clone(&f.revision),
                &f.resources,
                129,
                320,
            )
            .unwrap();
            assert_eq!(pcm16(&default), pcm16(&range)); // Real registered modules have zero algorithmic latency.
            let capture = directory.0.join("capture.wav");
            hex_export::render_prepared_controlled_with_device_resources(
                &capture,
                Arc::clone(&f.revision),
                &f.resources,
                0,
                640,
                ExportSignal::CapturedMix(vec![
                    OutputCaptureId::Track(f.sampler),
                    OutputCaptureId::Return(f.bus),
                ]),
                &ExportControl::default(),
            )
            .unwrap();
            let captured: Vec<_> = sampled
                .iter()
                .zip(&returned)
                .map(|(a, b)| [a[0] + b[0], a[1] + b[1]])
                .collect();
            assert_near(pcm16(&capture), &captured);
            let capture_range = directory.0.join("capture-range.wav");
            let report = hex_export::render_prepared_controlled_with_device_resources(
                &capture_range,
                Arc::clone(&f.revision),
                &f.resources,
                129,
                320,
                ExportSignal::CapturedMix(vec![
                    OutputCaptureId::Track(f.sampler),
                    OutputCaptureId::Return(f.bus),
                ]),
                &ExportControl::default(),
            )
            .unwrap();
            assert_eq!(report.frames, 320);
            assert_near(pcm16(&capture_range), &captured[129..449]);
            let stems = directory.0.join("stems");
            hex_export::render_stems_new_with_device_resources(
                &stems,
                Arc::clone(&f.revision),
                &f.resources,
                0,
                640,
                &[
                    Stem {
                        id: OutputCaptureId::Track(f.sampler),
                        name: "sampler".into(),
                    },
                    Stem {
                        id: OutputCaptureId::Return(f.bus),
                        name: "bus".into(),
                    },
                ],
                &ExportControl::default(),
            )
            .unwrap();
            assert_near(
                pcm16(&stems.join(format!("track-{}-sampler.wav", f.sampler.value()))),
                &sampled,
            );
            assert_near(
                pcm16(&stems.join(format!("return-{}-bus.wav", f.bus.value()))),
                &returned,
            );
            let bounce = directory.0.join("bounce.wav");
            hex_export::render_bounce_new_with_device_resources(
                &bounce,
                Arc::clone(&f.revision),
                &f.resources,
                129,
                320,
                2.0,
            )
            .unwrap();
            let bytes = fs::read(bounce).unwrap();
            assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 3);
            let actual: Vec<_> = bytes[hex_export::FLOAT32_WAV_HEADER_BYTES as usize..]
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
                .collect();
            for (actual, expected) in actual.into_iter().zip(expected[129..449].iter().flatten()) {
                assert!((actual - expected * 2.0).abs() < 1e-6);
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn resource_scope_cancel_range_and_preroll_refusals_leave_no_output() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let directory = Directory::new();
            let f = fixture(false);
            let foreign = fixture(false);
            assert!(
                SessionPlayback::new_linear_arrangement_export(Arc::clone(&f.revision)).is_err()
            );
            assert!(
                SessionPlayback::new_linear_arrangement_export_with_device_resources(
                    Arc::clone(&f.revision),
                    &foreign.resources
                )
                .is_err()
            );
            let path = directory.0.join("refused.wav");
            for (start, frames, resources, control) in [
                (0, 1, &foreign.resources, ExportControl::default()),
                (u64::MAX, 2, &f.resources, ExportControl::default()),
                (
                    hex_engine::MAX_ARRANGEMENT_PREROLL_FRAMES + 1,
                    1,
                    &f.resources,
                    ExportControl::default(),
                ),
                (0, 1, &f.resources, {
                    let control = ExportControl::default();
                    control.cancel();
                    control
                }),
            ] {
                assert!(
                    hex_export::render_prepared_controlled_with_device_resources(
                        &path,
                        Arc::clone(&f.revision),
                        resources,
                        start,
                        frames,
                        ExportSignal::Main,
                        &control
                    )
                    .is_err()
                );
                assert!(!path.exists());
                assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
            }
            assert!(hex_export::render_prepared_new(&path, Arc::clone(&f.revision), 0, 1).is_err());
            assert!(
                hex_export::render_bounce_new_with_device_resources(
                    &path,
                    Arc::clone(&f.revision),
                    &f.resources,
                    0,
                    1,
                    f32::NAN
                )
                .is_err()
            );
            let looped = fixture(true);
            assert!(
                hex_export::render_prepared_controlled_with_device_resources(
                    &path,
                    looped.revision,
                    &looped.resources,
                    129,
                    1,
                    ExportSignal::Main,
                    &ExportControl::default()
                )
                .is_err()
            );
            assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
        })
        .unwrap()
        .join()
        .unwrap();
}
