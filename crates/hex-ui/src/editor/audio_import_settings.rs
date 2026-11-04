//! Application import preference and admission-time intent capture. No project
//! mutation or decode lives here; the worker owns the one canonical transaction.
//! Loading blocks the toggle, so a late load cannot overwrite user intent. A
//! toggle becomes active only after its serialized background save succeeds;
//! queued imports retain their previous immutable policy while a save is pending.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_preferences::AudioImportPreferences;
use hex_project::AudioImportWarp;
use hex_session::{
    import_batch::AudioBatchDestination, import_batch_decode::AudioBatchContext, service::Request,
};
use std::path::PathBuf;

pub(super) struct State {
    path: PathBuf,
    preferences: AudioImportPreferences,
    loading: bool,
    saving: bool,
    writable: bool,
    error: Option<String>,
}
impl State {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn is_loading(&self) -> bool {
        self.loading
    }

    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            preferences: Default::default(),
            loading: true,
            saving: false,
            writable: true,
            error: None,
        }
    }
    fn policy(&self) -> AudioImportWarp {
        if self.preferences.auto_warp_beats {
            AudioImportWarp::Beats
        } else {
            AudioImportWarp::Off
        }
    }

    fn capture(&self, request: Request, context: AudioBatchContext) -> Result<Request, String> {
        let arrangement_import = match &request {
            Request::Import(_)
            | Request::ImportToTrack { .. }
            | Request::ImportToTrackWithOverwrite { .. }
            | Request::ImportToNewTrack { .. } => true,
            Request::ImportAudioBatch { destination, .. } => {
                !matches!(destination, AudioBatchDestination::Console(_))
            }
            _ => false,
        };
        if self.loading && arrangement_import {
            return Err("audio import settings are loading · retry the import".into());
        }
        capture(request, self.policy(), context)
    }
}

/// Capture enabled Arrangement import intent before queuing. Legacy Off imports
/// keep their existing FIFO/destination/overwrite contract and inherently remain
/// Off without any later preference read. Explicit batches retain their original
/// strict source context; Console and sampler requests are untouched.
fn capture(
    request: Request,
    warp: AudioImportWarp,
    context: AudioBatchContext,
) -> Result<Request, String> {
    let (path, destination, overwrite) = match request {
        Request::ImportAudioBatch {
            paths,
            destination,
            context,
            control,
            mut options,
        } => {
            if !matches!(destination, AudioBatchDestination::Console(_)) {
                options.warp = warp;
            }
            return Ok(Request::ImportAudioBatch {
                paths,
                destination,
                context,
                control,
                options,
            });
        }
        other if warp == AudioImportWarp::Off => return Ok(other),
        Request::Import(path) => (
            path,
            AudioBatchDestination::NewTracks {
                before_track: None,
                start: 0,
            },
            false,
        ),
        Request::ImportToTrack { path, track, start } => {
            (path, AudioBatchDestination::Track { track, start }, false)
        }
        Request::ImportToTrackWithOverwrite { path, track, start } => {
            (path, AudioBatchDestination::Track { track, start }, true)
        }
        Request::ImportToNewTrack {
            path,
            before_track,
            start,
        } => (
            path,
            AudioBatchDestination::NewTracks {
                before_track,
                start,
            },
            false,
        ),
        other => return Ok(other),
    };
    let (mut request, _) = Request::audio_batch(vec![path], destination, context)?;
    if let Request::ImportAudioBatch { options, .. } = &mut request {
        options.warp = warp;
        options.overwrite = overwrite;
    }
    Ok(request)
}

impl EditorView {
    pub(super) fn capture_audio_import_policy(&self, request: Request) -> Result<Request, String> {
        let mut request = self.audio_import_settings.capture(
            request,
            AudioBatchContext {
                project_generation: self.project_generation,
                revision: self.state.as_ref().map_or(0, |state| state.revision),
                active_gesture: self.device_gesture.snapshot().is_some(),
            },
        )?;
        if let Request::ImportAudioBatch {
            destination: AudioBatchDestination::NewTracks { .. },
            options,
            ..
        } = &mut request
        {
            options.new_track_gain =
                Some(self.track_defaults.captured_gain().map_err(str::to_owned)?);
        }
        Ok(request)
    }

    pub(super) fn start_audio_import_preferences_load(&mut self, cx: &mut Context<Self>) {
        let path = self.audio_import_settings.path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::load_audio_import_preferences(&path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.audio_import_settings;
                state.loading = false;
                match result {
                    Ok(preferences) => state.preferences = preferences,
                    Err(error) => {
                        state.writable = false;
                        state.error = Some(format!(
                            "could not load audio import settings · {}",
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string()
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_audio_import_warp(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.audio_import_settings;
        if state.loading || state.saving || !state.writable {
            return;
        }
        let preferences = AudioImportPreferences {
            auto_warp_beats: !state.preferences.auto_warp_beats,
        };
        state.saving = true;
        state.error = None;
        let path = state.path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::save_audio_import_preferences(&path, preferences)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.audio_import_settings;
                state.saving = false;
                match result {
                    Ok(()) => state.preferences = preferences,
                    Err(error) => {
                        state.error = Some(format!(
                            "could not save audio import settings · {}",
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string()
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn audio_import_settings_row(&self, cx: &mut Context<Self>) -> Div {
        let state = &self.audio_import_settings;
        let style = super::device_panel::control_style();
        let label = if state.loading {
            "auto-warp imported audio to beats · loading…"
        } else if state.saving {
            "auto-warp imported audio to beats · saving…"
        } else if state.preferences.auto_warp_beats {
            "auto-warp imported audio to beats · on"
        } else {
            "auto-warp imported audio to beats · off"
        };
        let mut row = div().flex().flex_col().gap(px(3.0)).child(
            div()
                .id("auto warp imports")
                .debug_selector(|| "auto warp imports".into())
                .text_xs()
                .px(px(4.0))
                .py(px(3.0))
                .border_1()
                .border_color(rgb(style.border))
                .cursor_pointer()
                .on_click(cx.listener(|view, _, _, cx| view.toggle_audio_import_warp(cx)))
                .child(theme::text(label)),
        );
        if let Some(error) = &state.error {
            row = row.child(
                div()
                    .id("audio import settings status")
                    .debug_selector(|| "audio import settings status".into())
                    .text_xs()
                    .child(theme::text(error)),
            );
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target_ids() -> (hex_project::TrackId, hex_project::SceneId) {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(hex_project::Command::AddTrack {
                name: "target".into(),
            })
            .unwrap();
        editor
            .execute(hex_project::Command::AddScene {
                name: "scene".into(),
            })
            .unwrap();
        (
            editor.project().tracks()[0].id(),
            editor.project().scenes()[0].id(),
        )
    }

    fn context() -> AudioBatchContext {
        AudioBatchContext {
            project_generation: 7,
            revision: 11,
            active_gesture: false,
        }
    }

    #[test]
    fn loading_never_captures_placeholder_policy_for_arrangement_imports() {
        let mut settings = State::new(PathBuf::new());
        let make_import = || Request::Import(PathBuf::from("/early.wav"));
        assert!(
            settings
                .capture(make_import(), context())
                .unwrap_err()
                .contains("loading")
        );
        let console = Request::ImportToSession {
            path: PathBuf::from("/console.wav"),
            track: target_ids().0,
            scene: target_ids().1,
        };
        assert!(matches!(
            settings.capture(console, context()).unwrap(),
            Request::ImportToSession { .. }
        ));
        settings.preferences.auto_warp_beats = true;
        settings.loading = false;
        assert!(
            matches!(settings.capture(make_import(), context()).unwrap(),
            Request::ImportAudioBatch { options, .. } if options.warp == AudioImportWarp::Beats)
        );
    }

    #[test]
    fn off_retains_legacy_requests_and_enabled_imports_capture_policy_before_queueing() {
        let (track, _) = target_ids();
        let path = PathBuf::from("/Original Case/sample.WAV");
        for enabled in [false, true] {
            let warp = if enabled {
                AudioImportWarp::Beats
            } else {
                AudioImportWarp::Off
            };
            let requests = [
                (Request::Import(path.clone()), false, 0),
                (
                    Request::ImportToTrack {
                        path: path.clone(),
                        track,
                        start: 99,
                    },
                    false,
                    99,
                ),
                (
                    Request::ImportToTrackWithOverwrite {
                        path: path.clone(),
                        track,
                        start: 100,
                    },
                    true,
                    100,
                ),
                (
                    Request::ImportToNewTrack {
                        path: path.clone(),
                        before_track: Some(track),
                        start: 101,
                    },
                    false,
                    101,
                ),
            ];
            for (request, overwrite, expected_start) in requests {
                let request = capture(request, warp, context()).unwrap();
                if !enabled {
                    let (actual_path, actual_start, actual_overwrite) = match request {
                        Request::Import(path) => (path, 0, false),
                        Request::ImportToTrack {
                            path,
                            track: target,
                            start,
                        } => {
                            assert_eq!(target, track);
                            (path, start, false)
                        }
                        Request::ImportToTrackWithOverwrite {
                            path,
                            track: target,
                            start,
                        } => {
                            assert_eq!(target, track);
                            (path, start, true)
                        }
                        Request::ImportToNewTrack {
                            path,
                            before_track,
                            start,
                        } => {
                            assert_eq!(before_track, Some(track));
                            (path, start, false)
                        }
                        _ => panic!("off must preserve the legacy admission contract"),
                    };
                    assert_eq!(actual_path, path);
                    assert_eq!(actual_start, expected_start);
                    assert_eq!(actual_overwrite, overwrite);
                    continue;
                }
                // A later preference change has no reference into the owned request.
                let Request::ImportAudioBatch {
                    paths,
                    destination,
                    context: actual,
                    options,
                    ..
                } = request
                else {
                    panic!("captured import missing");
                };
                assert_eq!(paths, vec![path.clone()]);
                assert_eq!(actual, context());
                assert_eq!(options.warp, warp);
                assert_eq!(options.overwrite, overwrite);
                let start = match destination {
                    AudioBatchDestination::Track {
                        start,
                        track: target,
                    } => {
                        assert_eq!(target, track);
                        start
                    }
                    AudioBatchDestination::NewTracks { start, .. } => start,
                    _ => panic!("unexpected console target"),
                };
                assert_eq!(start, expected_start);
            }
        }
        assert_eq!(State::new(PathBuf::new()).policy(), AudioImportWarp::Off);
    }

    #[test]
    fn queued_legacy_off_import_is_not_reinterpreted_when_preference_changes() {
        let mut settings = State::new(PathBuf::new());
        let request = capture(
            Request::ImportToTrackWithOverwrite {
                path: PathBuf::from("/queued.wav"),
                track: target_ids().0,
                start: 4321,
            },
            settings.policy(),
            context(),
        )
        .unwrap();
        settings.preferences.auto_warp_beats = true;
        assert_eq!(settings.policy(), AudioImportWarp::Beats);
        // The worker owns this legacy request and never reads application settings.
        assert!(
            matches!(request, Request::ImportToTrackWithOverwrite { path, track, start }
            if path == std::path::Path::new("/queued.wav") && track == target_ids().0 && start == 4321)
        );
    }

    #[test]
    fn multi_file_capture_preserves_order_control_and_console_negative_control() {
        let paths = vec![PathBuf::from("/B.WAV"), PathBuf::from("/a.wav")];
        let (request, control) = Request::audio_batch(
            paths.clone(),
            AudioBatchDestination::NewTracks {
                before_track: None,
                start: 7,
            },
            context(),
        )
        .unwrap();
        let request = capture(request, AudioImportWarp::Beats, context()).unwrap();
        let Request::ImportAudioBatch {
            paths: actual,
            options,
            control: actual_control,
            ..
        } = request
        else {
            panic!("batch missing");
        };
        assert_eq!(actual, paths);
        assert_eq!(options.warp, AudioImportWarp::Beats);
        assert!(std::sync::Arc::ptr_eq(&control, &actual_control));
        assert_eq!(
            control.request_cancel(),
            hex_session::import_batch_decode::AudioBatchCancel::Requested
        );
        assert!(actual_control.progress().cancellation_requested);
        let console = Request::ImportToSession {
            path: PathBuf::from("/a.wav"),
            track: target_ids().0,
            scene: target_ids().1,
        };
        assert!(matches!(
            capture(console, AudioImportWarp::Beats, context()).unwrap(),
            Request::ImportToSession { .. }
        ));
    }
}
