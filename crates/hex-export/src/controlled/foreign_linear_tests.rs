use super::*;
use hex_engine::{BorrowedMediaPage, MediaPages, StreamSourceKey};
use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;

use crate::latency_fixture;

struct Pages {
    prepared: usize,
    completed: usize,
    fail_complete: bool,
    fail_finish: bool,
    finished: bool,
}
impl MediaPages for Pages {
    fn page(&mut self, _: StreamSourceKey, _: u64) -> Option<BorrowedMediaPage<'_>> {
        None
    }
    fn request_page(&mut self, _: StreamSourceKey, _: u64) {}
}
impl PreparedExportPages for Pages {
    fn prepare_chunk(
        &mut self,
        _: &SessionPlayback,
        frames: usize,
        _: &ExportControl,
    ) -> Result<(), ExportError> {
        assert!(frames <= 8);
        self.prepared += 1;
        Ok(())
    }
    fn complete_chunk(&mut self) -> Result<(), ExportError> {
        self.completed += 1;
        if self.fail_complete {
            Err(ExportError::Invalid("page read failed"))
        } else {
            Ok(())
        }
    }
    fn finish(&mut self, _: &ExportControl) -> Result<(), ExportError> {
        self.finished = true;
        if self.fail_finish {
            Err(ExportError::Invalid("page verification failed"))
        } else {
            Ok(())
        }
    }
}
struct Provider {
    quantum: usize,
    frames: usize,
    fail_block: bool,
    cancel_block: bool,
    fail_finish: bool,
    finished: bool,
}
impl crate::ForeignBounceRenderer for Provider {
    fn max_block_frames(&self) -> usize {
        self.quantum
    }
    fn render_block(
        &mut self,
        _: &mut SessionPlayback,
        output: &mut [Frame],
        pages: Option<&mut dyn MediaPages>,
        control: &ExportControl,
    ) -> Result<(), ExportError> {
        assert!(pages.is_some());
        assert!(output.len() <= 8);
        if self.fail_block {
            return Err(ExportError::Invalid("provider render failed"));
        }
        if self.cancel_block {
            control.cancel();
            return Err(ExportError::Cancelled);
        }
        for frame in output {
            self.frames += 1;
            *frame = [self.frames as f32 / 32.0, 0.0];
        }
        Ok(())
    }
    fn finish(&mut self, _: &ExportControl) -> Result<(), ExportError> {
        self.finished = true;
        if self.fail_finish {
            Err(ExportError::Invalid("provider failed"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn linear_foreign_export_verifies_pages_and_provider_before_publication() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 64,
            playback_frames: 64,
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
            state: latency_fixture::Definition.default_state().unwrap(),
        })
        .unwrap();
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&latency_fixture::Definition];
    let effects = NativeDeviceRegistry::new(&definitions).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_effect_registry(
            editor.project(),
            &BTreeMap::from([(
                asset,
                hex_engine::Audio::new(48_000, vec![[0.0; 2]; 64]).unwrap(),
            )]),
            Arc::new(hex_engine::SineTable::new()),
            None,
            &effects,
        )
        .unwrap(),
    );
    let dir = std::env::temp_dir().join(format!("hex-linear-foreign-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("mix.wav");
    for (page_read_fail, page_fail, render_fail, cancel, provider_fail, long_range) in [
        (true, false, false, false, false, false),
        (false, true, false, false, false, false),
        (false, false, true, false, false, false),
        (false, false, false, true, false, false),
        (false, false, false, false, true, false),
        (false, false, true, false, false, true),
        (false, false, false, false, false, false),
    ] {
        std::fs::write(&path, b"original").unwrap();
        let publication = Publication::confirmed_save(&path).unwrap();
        let mut pages = Pages {
            prepared: 0,
            completed: 0,
            fail_complete: page_read_fail,
            fail_finish: page_fail,
            finished: false,
        };
        let mut provider = Provider {
            quantum: 8,
            frames: 0,
            fail_block: render_fail,
            cancel_block: cancel,
            fail_finish: provider_fail,
            finished: false,
        };
        let frames = if long_range {
            hex_engine::session_playback::MAX_FINITE_BOUNCE_FRAMES + 1
        } else {
            19
        };
        let result = render_linear_main_with_foreign(
            ForeignLinearExportSpec {
                path: &path,
                revision: Arc::clone(&revision),
                resources: None,
                start: 5,
                frames,
                format: ExportFormat::Float32,
                control: &ExportControl::default(),
                publication: &publication,
            },
            Some(&mut pages),
            &mut provider,
        );
        if page_read_fail || page_fail || render_fail || cancel || provider_fail {
            assert!(result.is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"original");
            if long_range {
                assert!(pages.prepared > 0, "long export must reach rendering");
            }
        } else {
            assert!(pages.finished);
            assert_eq!(pages.prepared, pages.completed);
            assert!(provider.finished);
            assert_eq!(result.unwrap().frames, 19);
            let wav = std::fs::read(&path).unwrap();
            let samples: Vec<_> = wav[crate::FLOAT32_WAV_HEADER_BYTES as usize..]
                .chunks_exact(8)
                .map(|frame| f32::from_le_bytes(frame[..4].try_into().unwrap()))
                .collect();
            assert_eq!(
                samples,
                (13..=31).map(|n| n as f32 / 32.0).collect::<Vec<_>>()
            );
        }
    }
    let mut provider = Provider {
        quantum: 0,
        frames: 0,
        fail_block: false,
        cancel_block: false,
        fail_finish: false,
        finished: false,
    };
    assert!(matches!(
        render_linear_main_with_foreign(
            ForeignLinearExportSpec {
                path: &path,
                revision,
                resources: None,
                start: 0,
                frames: 19,
                format: ExportFormat::Float32,
                control: &ExportControl::default(),
                publication: &Publication::CreateNew,
            },
            None,
            &mut provider,
        ),
        Err(ExportError::Invalid(
            "invalid foreign export block or range"
        ))
    ));
    assert_eq!(provider.frames, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

struct CapturedForeignProvider {
    quantum: usize,
    realtime_only: bool,
    fail_block: bool,
    cancel_block: bool,
    fail_finish: bool,
    finished: bool,
    blocks: usize,
}

struct GainForeignProcessors(f32);

impl hex_lattice::ForeignProcessors for GainForeignProcessors {
    fn process_segment(
        &mut self,
        _node: hex_lattice::ForeignProcessorNode<'_>,
        _frame_offset: usize,
        frames: &mut [[f32; 2]],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        for frame in frames {
            frame[0] *= self.0;
            frame[1] *= self.0;
        }
        Ok(())
    }

    fn reject_callback(&mut self) {}
}

impl crate::ForeignBounceRenderer for CapturedForeignProvider {
    fn max_block_frames(&self) -> usize {
        self.quantum
    }
    fn requires_realtime_fallback(&self) -> bool {
        self.realtime_only
    }
    fn render_block(
        &mut self,
        playback: &mut SessionPlayback,
        output: &mut [Frame],
        pages: Option<&mut dyn MediaPages>,
        control: &ExportControl,
    ) -> Result<(), ExportError> {
        assert!(output.len() <= self.quantum);
        self.blocks += 1;
        if self.fail_block {
            return Err(ExportError::Invalid("provider render failed"));
        }
        if self.cancel_block {
            control.cancel();
            return Err(ExportError::Cancelled);
        }
        let mut fx = GainForeignProcessors(0.5);
        playback
            .render_with_foreign_processors(output, pages, None, None, &mut fx)
            .map_err(ExportError::SessionRender)
    }
    fn render_block_with_capture(
        &mut self,
        playback: &mut SessionPlayback,
        output: &mut [Frame],
        pages: Option<&mut dyn MediaPages>,
        capture: &mut OutputCapture<'_, '_>,
        control: &ExportControl,
    ) -> Result<(), ExportError> {
        assert!(output.len() <= self.quantum);
        self.blocks += 1;
        if self.fail_block {
            return Err(ExportError::Invalid("provider capture render failed"));
        }
        if self.cancel_block {
            control.cancel();
            return Err(ExportError::Cancelled);
        }
        let mut fx = GainForeignProcessors(0.5);
        playback
            .render_with_foreign_processors_and_capture(output, pages, capture, &mut fx)
            .map_err(ExportError::SessionRender)
    }
    fn finish(&mut self, _: &ExportControl) -> Result<(), ExportError> {
        self.finished = true;
        if self.fail_finish {
            Err(ExportError::Invalid("provider finish failed"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn foreign_export_supports_captured_mix_stems_and_realtime_fallback_refusal() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported {
        track: track_a,
        asset: asset_a,
        ..
    } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "track-a".into(),
            path: "a.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 64,
            playback_frames: 64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import a")
    };
    let Created::Imported {
        track: track_b,
        asset: asset_b,
        ..
    } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "track-b".into(),
            path: "b.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 64,
            playback_frames: 64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import b")
    };
    let Created::ReturnTrack(ret) = editor
        .execute(Command::AddReturnTrack {
            name: "verb".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track: track_a,
            return_track: ret,
            gain: 0.25,
        })
        .unwrap();
    // Tempo map change exercised alongside captured foreign export.
    editor
        .execute(Command::SetTempoPoint {
            beat: hex_project::BeatTime::from_quarters(1).unwrap(),
            tempo: hex_project::Tempo::new(132_000_000).unwrap(),
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([
                (
                    asset_a,
                    hex_engine::Audio::new(48_000, vec![[0.4, -0.4]; 64]).unwrap(),
                ),
                (
                    asset_b,
                    hex_engine::Audio::new(48_000, vec![[0.2, 0.2]; 64]).unwrap(),
                ),
            ]),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );

    let dir =
        std::env::temp_dir().join(format!("hex-foreign-captured-stems-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // 1. Truthful real-time fallback refusal before touching any file.
    let rt_path = dir.join("realtime.wav");
    std::fs::write(&rt_path, b"untouched").unwrap();
    let mut rt_provider = CapturedForeignProvider {
        quantum: 8,
        realtime_only: true,
        fail_block: false,
        cancel_block: false,
        fail_finish: false,
        finished: false,
        blocks: 0,
    };
    let rt_pub = Publication::confirmed_save(&rt_path).unwrap();
    assert!(matches!(
        render_prepared_with_foreign(
            ExportJobSpec {
                path: &rt_path,
                revision: Arc::clone(&revision),
                resources: None,
                start: 0,
                frames: 16,
                signal: ExportSignal::CapturedMix(vec![OutputCaptureId::Track(track_a)]),
                format: ExportFormat::Float32,
                control: &ExportControl::default(),
                publication: &rt_pub,
            },
            None,
            &mut rt_provider,
        ),
        Err(ExportError::Invalid(
            "offline vst3 plugin requires real-time render fallback"
        ))
    ));
    assert_eq!(rt_provider.blocks, 0);
    assert_eq!(std::fs::read(&rt_path).unwrap(), b"untouched");

    // 2. Solo-source CapturedMix for track_a only (excludes track_b).
    let solo_path = dir.join("solo-a.wav");
    let mut provider = CapturedForeignProvider {
        quantum: 8,
        realtime_only: false,
        fail_block: false,
        cancel_block: false,
        fail_finish: false,
        finished: false,
        blocks: 0,
    };
    let report = render_prepared_with_foreign(
        ExportJobSpec {
            path: &solo_path,
            revision: Arc::clone(&revision),
            resources: None,
            start: 0,
            frames: 19,
            signal: ExportSignal::CapturedMix(vec![OutputCaptureId::Track(track_a)]),
            format: ExportFormat::Float32,
            control: &ExportControl::default(),
            publication: &Publication::CreateNew,
        },
        None,
        &mut provider,
    )
    .unwrap();
    assert!(provider.finished);
    assert_eq!(report.frames, 19);
    let wav = std::fs::read(&solo_path).unwrap();
    let frames: Vec<[f32; 2]> = wav[crate::FLOAT32_WAV_HEADER_BYTES as usize..]
        .chunks_exact(8)
        .map(|chunk| {
            [
                f32::from_le_bytes(chunk[0..4].try_into().unwrap()),
                f32::from_le_bytes(chunk[4..8].try_into().unwrap()),
            ]
        })
        .collect();
    assert_eq!(frames.len(), 19);
    for frame in frames {
        assert!((frame[0] - 0.4).abs() < 1e-5);
        assert!((frame[1] + 0.4).abs() < 1e-5);
    }

    // 3. Stems batch export for track_a, track_b, and return bus.
    let stems_dir = dir.join("stems-batch");
    let mut stem_provider = CapturedForeignProvider {
        quantum: 8,
        realtime_only: false,
        fail_block: false,
        cancel_block: false,
        fail_finish: false,
        finished: false,
        blocks: 0,
    };
    let stems = vec![
        crate::Stem {
            id: OutputCaptureId::Track(track_a),
            name: "track-a".into(),
        },
        crate::Stem {
            id: OutputCaptureId::Track(track_b),
            name: "track-b".into(),
        },
        crate::Stem {
            id: OutputCaptureId::Return(ret),
            name: "verb".into(),
        },
    ];
    let stem_report = render_prepared_with_foreign(
        ExportJobSpec {
            path: &stems_dir,
            revision: Arc::clone(&revision),
            resources: None,
            start: 0,
            frames: 16,
            signal: ExportSignal::Stems(stems.clone()),
            format: ExportFormat::Float32,
            control: &ExportControl::default(),
            publication: &Publication::CreateNew,
        },
        None,
        &mut stem_provider,
    )
    .unwrap();
    assert!(stem_provider.finished);
    assert_eq!(stem_report.frames, 16);
    assert!(stems_dir.is_dir());
    let mut stem_files: Vec<_> = std::fs::read_dir(&stems_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    stem_files.sort();
    assert_eq!(stem_files.len(), 3);

    // 4. Stems failure during finish cleans staging without publishing destination.
    let failed_stems_dir = dir.join("stems-failed");
    let mut failing_provider = CapturedForeignProvider {
        quantum: 8,
        realtime_only: false,
        fail_block: false,
        cancel_block: false,
        fail_finish: true,
        finished: false,
        blocks: 0,
    };
    assert!(
        render_prepared_with_foreign(
            ExportJobSpec {
                path: &failed_stems_dir,
                revision,
                resources: None,
                start: 0,
                frames: 16,
                signal: ExportSignal::Stems(stems),
                format: ExportFormat::Float32,
                control: &ExportControl::default(),
                publication: &Publication::CreateNew,
            },
            None,
            &mut failing_provider,
        )
        .is_err()
    );
    assert!(failing_provider.finished);
    assert!(!failed_stems_dir.exists());

    std::fs::remove_dir_all(dir).unwrap();
}
