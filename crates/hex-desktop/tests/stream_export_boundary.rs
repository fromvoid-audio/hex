//! Real Session export and page-worker oracle in a small test executable.
//!
//! Include the desktop owners directly so these integration cases use the same
//! source and reservation as the application without loading the UI test binary.
//! Their unrelated entry points are unused in this executable.
#[allow(dead_code)]
#[path = "../src/audio_policy.rs"]
mod audio_policy;
#[allow(dead_code)]
#[path = "../src/granular_sources.rs"]
mod granular_sources;
#[path = "stream_export_boundary/long_stretch.rs"]
mod long_stretch;
#[allow(dead_code)]
#[path = "../src/streamed_export.rs"]
mod streamed_export;
use audio_policy::AudioPolicy;
use hex_session::{
    DocumentAudioMode,
    service::{CanonicalAudioSourceSnapshot, Event, Request, Service, WorkerAudioSink},
};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64},
        mpsc,
    },
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-desktop-worker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn wav(&self, rate: u32, frames: u32) -> PathBuf {
        let path = self.0.join("mixed-rate.wav");
        let mut bytes = Vec::from(*b"RIFF");
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        for _ in 0..frames {
            bytes.extend_from_slice(&8192i16.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct OfflineSink {
    rate: u32,
    render: hex_session::service::StreamedExporter,
}
impl WorkerAudioSink for OfflineSink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(self.rate)
    }
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn poll(&mut self) {}
    fn close(&mut self) {}
    fn streamed_exporter(&self) -> Option<hex_session::service::StreamedExporter> {
        Some(self.render)
    }
    fn streamed_bouncer(&self) -> Option<hex_session::StreamedBouncer> {
        Some(streamed_export::bounce)
    }
}
fn offline(rate: u32, render: hex_session::service::StreamedExporter) -> Service {
    let service =
        Service::with_audio_sink(16, 0, None, Box::new(OfflineSink { rate, render })).unwrap();
    assert!(event(&service).error.is_none());
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        },
    );
    service
}
fn event(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(Instant::now() < deadline, "source worker stalled");
        std::thread::yield_now();
    }
}
fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn exported(service: &Service) -> hex_session::service::ExportEvent {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match service.try_export_event() {
            Ok(event) => return event,
            Err(mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < deadline, "offline export stalled");
                std::thread::yield_now();
            }
            Err(error) => panic!("export channel: {error}"),
        }
    }
}
fn varied(files: &Files, name: &str, rate: u32, frames: u32) -> PathBuf {
    let base = files.wav(rate, frames);
    let mut bytes = fs::read(base).unwrap();
    for (i, frame) in bytes[44..].chunks_exact_mut(2).enumerate() {
        let value = ((i as i32 * 73) % 40_000 - 20_000) as i16;
        frame.copy_from_slice(&value.to_le_bytes());
    }
    let path = files.0.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

#[cfg(feature = "allocation-probe")]
#[test]
fn verified_stem_page_miss_removes_the_entire_unpublished_batch() {
    use hex_session::service::{ExportControl, ExportSignal, Stem};

    let files = Files::new();
    let a = varied(&files, "a.wav", 48_000, 2048);
    let b = varied(&files, "b.wav", 48_000, 2048);
    let originals = [fs::read(&a).unwrap(), fs::read(&b).unwrap()];
    let neighbor = files.0.join("unrelated");
    fs::write(&neighbor, b"preserve").unwrap();
    let before: std::collections::BTreeSet<_> = fs::read_dir(&files.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();

    let service = offline(48_000, crate::streamed_export::test_support::missing_page);
    let mut stems = Vec::new();
    for (path, name) in [(&a, "a"), (&b, "b")] {
        let imported = run(&service, Request::Import(path.clone()));
        let track = imported.state.project.tracks().last().unwrap().id();
        stems.push(Stem {
            id: hex_engine::OutputCaptureId::Track(track),
            name: name.into(),
        });
    }
    let output = files.0.join("batch");
    run(
        &service,
        Request::ExportRange {
            path: output.clone(),
            start: 0,
            frames: Some(1024),
            signal: ExportSignal::Stems(stems),
            control: ExportControl::default(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: false,
        },
    );
    let error = exported(&service).result.unwrap_err();
    assert!(
        error.message().contains("unexpected media page miss"),
        "{error}"
    );
    assert!(
        error.source_error().is_some(),
        "streamed publisher error remains structured"
    );
    assert!(!output.exists());
    let after: std::collections::BTreeSet<_> = fs::read_dir(&files.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(after, before, "all task-owned staging must be removed");
    assert_eq!(fs::read(&a).unwrap(), originals[0]);
    assert_eq!(fs::read(&b).unwrap(), originals[1]);
    assert_eq!(fs::read(neighbor).unwrap(), b"preserve");
}

#[test]
fn verified_export_44k_ungrouped_main_matches_resident_pcm() {
    assert_verified_export_matches_resident(44100, false, 0);
}

#[test]
fn verified_export_44k_ungrouped_capture_matches_resident_pcm() {
    assert_verified_export_matches_resident(44100, false, 1);
}

#[test]
fn verified_export_44k_ungrouped_stems_matches_resident_pcm() {
    assert_verified_export_matches_resident(44100, false, 2);
}

#[test]
fn verified_export_48k_ungrouped_main_matches_resident_pcm() {
    assert_verified_export_matches_resident(48000, false, 0);
}

#[test]
fn verified_export_48k_ungrouped_capture_matches_resident_pcm() {
    assert_verified_export_matches_resident(48000, false, 1);
}

#[test]
fn verified_export_48k_ungrouped_stems_matches_resident_pcm() {
    assert_verified_export_matches_resident(48000, false, 2);
}

#[test]
fn verified_export_44k_nested_groups_main_matches_resident_pcm() {
    assert_verified_export_matches_resident(44100, true, 0);
}

#[test]
fn verified_export_44k_nested_groups_capture_matches_resident_pcm() {
    assert_verified_export_matches_resident(44100, true, 1);
}

#[test]
fn verified_export_44k_nested_groups_stems_matches_resident_pcm() {
    assert_verified_export_matches_resident(44100, true, 2);
}

#[test]
fn verified_export_48k_nested_groups_main_matches_resident_pcm() {
    assert_verified_export_matches_resident(48000, true, 0);
}

#[test]
fn verified_export_48k_nested_groups_capture_matches_resident_pcm() {
    assert_verified_export_matches_resident(48000, true, 1);
}

#[test]
fn verified_export_48k_nested_groups_stems_matches_resident_pcm() {
    assert_verified_export_matches_resident(48000, true, 2);
}

fn assert_verified_export_matches_resident(rate: u32, grouped: bool, signal_index: usize) {
    use hex_project::{Command, Created, TrimEdge};
    use hex_session::service::{ExportControl, ExportSignal, Stem};
    let files = Files::new();
    let a = varied(&files, "a.wav", 24000, 8192);
    let b = varied(&files, "b.wav", 48000, 12288);
    let bytes = [fs::read(&a).unwrap(), fs::read(&b).unwrap()];
    let streamed = offline(rate, crate::streamed_export::render);
    let resident = Service::new(rate, 16, 100000).unwrap();
    event(&resident);
    let mut scenes = Vec::new();
    for service in [&streamed, &resident] {
        let mut tracks = Vec::new();
        for path in [&a, &b] {
            let imported = run(service, Request::Import(path.clone()));
            tracks.push(imported.state.project.tracks().last().unwrap().id());
        }
        let state = run(
            service,
            Request::Edit(Command::Gain {
                track: tracks[0],
                gain: 0.6,
            }),
        );
        let clip = state.state.project.track(tracks[0]).unwrap().clips()[0].id();
        run(
            service,
            Request::Edit(Command::SetClipLooping {
                clip,
                looping: true,
            }),
        );
        run(
            service,
            Request::Edit(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Right,
                frame: 40000,
            }),
        );
        let returned = run(
            service,
            Request::Edit(Command::AddReturnTrack { name: "bus".into() }),
        );
        let Created::ReturnTrack(bus) = returned.created.unwrap() else {
            panic!("return");
        };
        run(
            service,
            Request::Edit(Command::SetReturnSend {
                track: tracks[0],
                return_track: bus,
                gain: 0.5,
            }),
        );
        run(
            service,
            Request::Edit(Command::SetReturnSend {
                track: tracks[1],
                return_track: bus,
                gain: 0.25,
            }),
        );
        run(
            service,
            Request::Edit(Command::ReturnGain {
                return_track: bus,
                gain: 0.8,
            }),
        );
        run(
            service,
            Request::AddNativeDevice {
                track: tracks[0],
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.utility")),
                before: None,
            },
        );
        if grouped {
            let group = run(
                service,
                Request::Edit(Command::GroupTracks {
                    tracks: hex_project::TrackSelectionIds::new(&tracks).unwrap(),
                    name: "group".into(),
                }),
            );
            let Created::Track(inner) = group.created.unwrap() else {
                panic!("group")
            };
            run(
                service,
                Request::Edit(Command::Gain {
                    track: inner,
                    gain: 0.5,
                }),
            );
            let group = run(
                service,
                Request::Edit(Command::GroupTracks {
                    tracks: hex_project::TrackSelectionIds::new(&[inner]).unwrap(),
                    name: "outer".into(),
                }),
            );
            let Created::Track(outer) = group.created.unwrap() else {
                panic!("outer group")
            };
            run(
                service,
                Request::Edit(Command::Gain {
                    track: outer,
                    gain: 0.75,
                }),
            );
        }
        let state = run(service, Request::Edit(Command::MasterGain { gain: 0.5 }));
        scenes.push((tracks[0], bus, state));
    }
    for (index, signal) in [
        ExportSignal::Main,
        ExportSignal::CapturedMix(vec![
            hex_engine::OutputCaptureId::Track(scenes[0].0),
            hex_engine::OutputCaptureId::Return(scenes[0].1),
        ]),
        ExportSignal::Stems(vec![
            Stem {
                id: hex_engine::OutputCaptureId::Track(scenes[0].0),
                name: "a".into(),
            },
            Stem {
                id: hex_engine::OutputCaptureId::Return(scenes[0].1),
                name: "bus".into(),
            },
        ]),
    ]
    .into_iter()
    .enumerate()
    .filter(|(index, _)| *index == signal_index)
    {
        let mut outputs = Vec::new();
        let mut controls = Vec::new();
        for (i, service) in [&streamed, &resident].into_iter().enumerate() {
            assert_eq!(
                (scenes[0].0, scenes[0].1),
                (scenes[1].0, scenes[1].1),
                "model identities independent of source storage"
            );
            let path = files.0.join(format!("{index}-{i}"));
            let control = ExportControl::default();
            let queued = run(
                service,
                Request::ExportRange {
                    path: path.clone(),
                    start: 137,
                    frames: Some(18000),
                    signal: signal.clone(),
                    control: control.clone(),
                    format: hex_export::ExportFormat::Pcm16,
                    overwrite: false,
                },
            );
            assert_eq!(queued.state.project, scenes[i].2.state.project);
            assert_eq!(queued.state.unsaved, scenes[i].2.state.unsaved);
            outputs.push(path);
            controls.push(control);
        }
        // Separate services and output paths allow the two real jobs to run
        // concurrently; both complete before any files are compared.
        for (i, service) in [&streamed, &resident].into_iter().enumerate() {
            let completed = exported(service);
            assert!(completed.result.is_ok(), "{:?}", completed.result);
            assert_eq!(completed.revision, scenes[i].2.state.revision);
            assert_eq!(
                controls[i].progress().processed_frames,
                controls[i].progress().total_frames
            );
        }
        if matches!(signal, ExportSignal::Stems(_)) {
            let names: Vec<_> = fs::read_dir(&outputs[0])
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert_eq!(names.len(), 2);
            for name in names {
                assert_eq!(
                    fs::read(outputs[0].join(&name)).unwrap(),
                    fs::read(outputs[1].join(name)).unwrap()
                );
            }
        } else {
            assert_eq!(
                fs::read(&outputs[0]).unwrap(),
                fs::read(&outputs[1]).unwrap(),
                "source storage does not alter PCM16"
            );
        }
    }
    assert_eq!(fs::read(a).unwrap(), bytes[0]);
    assert_eq!(fs::read(b).unwrap(), bytes[1]);
}

use hex_stream::StreamConfig;
fn tiny(
    request: &hex_session::service::StreamedExportRequest<'_>,
) -> Result<hex_export::ExportReport, hex_session::service::ExportFailure> {
    crate::streamed_export::render_with_config(request, StreamConfig::with_pool_pages(3))
        .map_err(Into::into)
}

struct OfflinePassthrough {
    fail: bool,
    frames: usize,
}
impl hex_export::ForeignBounceRenderer for OfflinePassthrough {
    fn max_block_frames(&self) -> usize {
        64
    }
    fn render_block(
        &mut self,
        playback: &mut hex_engine::SessionPlayback,
        output: &mut [hex_engine::Frame],
        pages: Option<&mut dyn hex_engine::MediaPages>,
        _: &hex_export::ExportControl,
    ) -> Result<(), hex_export::ExportError> {
        assert!(output.len() <= 64);
        self.frames += output.len();
        if self.fail {
            return Err(hex_export::ExportError::Invalid("test processor failed"));
        }
        playback
            .render_with_pages_and_post_node_observer(
                output,
                pages.ok_or(hex_export::ExportError::Invalid("missing pages"))?,
                None,
            )
            .map_err(hex_export::ExportError::SessionRender)
    }
    fn finish(&mut self, _: &hex_export::ExportControl) -> Result<(), hex_export::ExportError> {
        assert!(self.frames > 0);
        Ok(())
    }
}
fn foreign_export(
    request: &hex_session::service::StreamedExportRequest<'_>,
) -> Result<hex_export::ExportReport, hex_session::service::ExportFailure> {
    crate::streamed_export::render_with_config_and_foreign(
        request,
        StreamConfig::with_pool_pages(3),
        Some(&mut OfflinePassthrough {
            fail: false,
            frames: 0,
        }),
    )
    .map_err(Into::into)
}
fn failing_foreign_export(
    request: &hex_session::service::StreamedExportRequest<'_>,
) -> Result<hex_export::ExportReport, hex_session::service::ExportFailure> {
    crate::streamed_export::render_with_config_and_foreign(
        request,
        StreamConfig::with_pool_pages(3),
        Some(&mut OfflinePassthrough {
            fail: true,
            frames: 0,
        }),
    )
    .map_err(Into::into)
}

#[test]
fn verified_pages_feed_offline_processor_and_failures_preserve_existing_output() {
    use hex_session::service::{ExportControl, ExportSignal};
    let files = Files::new();
    let source = varied(&files, "foreign-source.wav", 48000, 12288);
    let original = fs::read(&source).unwrap();
    let foreign = offline(48000, foreign_export);
    let imported = run(&foreign, Request::Import(source.clone()));
    let track = imported.state.project.tracks()[0].id();
    let output = files.0.join("foreign.wav");
    run(&foreign, Request::Export(output.clone()));
    assert!(exported(&foreign).result.is_ok());
    let ordinary = offline(48000, tiny);
    run(&ordinary, Request::Import(source.clone()));
    let reference = files.0.join("ordinary.wav");
    run(&ordinary, Request::Export(reference.clone()));
    assert!(exported(&ordinary).result.is_ok());
    assert_eq!(fs::read(&output).unwrap(), fs::read(reference).unwrap());
    let existing = files.0.join("existing-foreign.wav");
    let failing = offline(48000, failing_foreign_export);
    run(&failing, Request::Import(source.clone()));
    fs::write(&existing, b"keep existing").unwrap();
    let request = || Request::ExportRange {
        path: existing.clone(),
        start: 0,
        frames: None,
        signal: ExportSignal::Main,
        control: ExportControl::default(),
        format: hex_export::ExportFormat::Pcm16,
        overwrite: true,
    };
    run(&failing, request());
    assert!(exported(&failing).result.is_err());
    assert_eq!(fs::read(&existing).unwrap(), b"keep existing");
    let mut changed = original.clone();
    changed[48] ^= 1;
    fs::write(&source, changed).unwrap();
    run(&foreign, request());
    assert!(exported(&foreign).result.is_err());
    assert_eq!(fs::read(&existing).unwrap(), b"keep existing");
    fs::remove_file(&source).unwrap();
    run(&foreign, request());
    assert!(exported(&foreign).result.is_err());
    assert_eq!(fs::read(&existing).unwrap(), b"keep existing");
    fs::write(&source, &original).unwrap();
    run(
        &foreign,
        Request::ExportRange {
            path: existing.clone(),
            start: 0,
            frames: None,
            signal: ExportSignal::CapturedMix(vec![hex_engine::OutputCaptureId::Track(track)]),
            control: ExportControl::default(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: true,
        },
    );
    let error = exported(&foreign).result.unwrap_err();
    assert!(error.message().contains("supports main only"), "{error}");
    assert_eq!(fs::read(existing).unwrap(), b"keep existing");
}

#[test]
fn granular_tones_export_matches_converted_stereo_resident_pcm() {
    granular_export(hex_project::WarpMode::Tones { grain_ms: 32 });
}
#[test]
fn granular_texture_export_matches_converted_stereo_resident_pcm() {
    granular_export(hex_project::WarpMode::Texture {
        grain_ms: 32,
        fluctuation: 35,
        seed: 42,
    });
}
fn granular_export(mode: hex_project::WarpMode) {
    let files = Files::new();
    let original = files.0.join("stereo-granular.wav");
    let mut frame = 0usize;
    hex_export::write_float_wav_new(&original, 24_000, 8192, &AtomicBool::new(false), |output| {
        for dst in output {
            let x = (frame as f32 * 0.071).sin() * 0.2;
            *dst = [x, x * -1.7];
            frame += 1;
        }
        Ok(())
    })
    .unwrap();
    let bytes = fs::read(&original).unwrap();
    fn small(
        request: &hex_session::service::StreamedExportRequest<'_>,
    ) -> Result<hex_export::ExportReport, hex_session::service::ExportFailure> {
        streamed_export::render_with_config(request, hex_stream::StreamConfig::with_pool_pages(4))
            .map_err(Into::into)
    }
    let streamed = offline(48_000, small);
    let resident = Service::new(48_000, 16, 65_536).unwrap();
    event(&resident);
    for service in [&streamed, &resident] {
        let imported = run(service, Request::Import(original.clone()));
        let clip = imported.state.project.tracks()[0].clips()[0].id();
        let neutral = run(
            service,
            Request::Edit(hex_project::Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            }),
        );
        let warp = hex_project::default_audio_warp(
            neutral.state.project.clip(clip).unwrap(),
            neutral.state.project.tempo_map(),
            mode,
        )
        .unwrap();
        run(
            service,
            Request::Edit(hex_project::Command::SetAudioClipWarp {
                clip,
                settings: Some(warp),
            }),
        );
        run(
            service,
            Request::Edit(hex_project::Command::SetProjectTempo {
                tempo: hex_project::Tempo::new(180_000_000).unwrap(),
            }),
        );
    }
    let actual = files.0.join("granular-streamed.wav");
    let expected = files.0.join("granular-resident.wav");
    for (service, path) in [(&streamed, &actual), (&resident, &expected)] {
        run(
            service,
            Request::ExportRange {
                path: path.clone(),
                start: 31,
                frames: Some(8192),
                overwrite: false,
                signal: hex_export::ExportSignal::Main,
                format: hex_export::ExportFormat::Pcm16,
                control: hex_export::ExportControl::default(),
            },
        );
        let finished = exported(service);
        assert!(finished.result.is_ok(), "{:?}", finished.result);
    }
    let decode = |path: &std::path::Path| {
        hex_media::decode_wav_file(path, hex_media::Limits::new(1 << 20, 65_536))
            .unwrap()
            .audio()
            .frames()
            .to_vec()
    };
    let actual = decode(&actual);
    let expected = decode(&expected);
    assert_eq!(actual.len(), 8192);
    assert_eq!(actual.len(), expected.len());
    assert!(expected.iter().flatten().any(|x| x.abs() > 0.01));
    let error = actual
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        error <= 1.0 / 32768.0,
        "granular export PCM difference {error}"
    );
    assert_eq!(fs::read(&original).unwrap(), bytes);
    let mut changed = bytes;
    *changed.last_mut().unwrap() ^= 1;
    fs::write(&original, &changed).unwrap();
    let refused = files.0.join("refused-granular.wav");
    run(
        &streamed,
        Request::ExportRange {
            path: refused.clone(),
            start: 31,
            frames: Some(8192),
            overwrite: false,
            signal: hex_export::ExportSignal::Main,
            format: hex_export::ExportFormat::Pcm16,
            control: hex_export::ExportControl::default(),
        },
    );
    assert!(exported(&streamed).result.is_err());
    assert!(
        !refused.exists(),
        "changed granular media must not publish output"
    );
    assert_eq!(fs::read(&original).unwrap(), changed);
}
