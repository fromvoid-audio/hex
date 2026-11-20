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
            &path,
            Arc::clone(&revision),
            None,
            5,
            frames,
            ExportFormat::Float32,
            &ExportControl::default(),
            &publication,
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
            &path,
            revision,
            None,
            0,
            19,
            ExportFormat::Float32,
            &ExportControl::default(),
            &Publication::CreateNew,
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
