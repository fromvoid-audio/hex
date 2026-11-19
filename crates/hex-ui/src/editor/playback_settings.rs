//! Application playback policy and one bounded normal-start slot during startup.
//! Raw Cue/Seek, history and structural resume retain their independent laws.
//! The UI captures targets before waiting; late loading cannot retarget a start.
use super::{EditorView, arrangement_seek::SeekIntent};
use gpui::Context;
use hex_audio::prepared::AppliedAudioSource;
use hex_engine::NoteChase;
use hex_preferences::PlaybackPreferences;
use std::path::PathBuf;

pub(super) enum Start {
    Session {
        command: hex_engine::SessionPlaybackCommand,
        revision: u64,
    },
    Play {
        frame: u64,
    },
    Source {
        frame: u64,
        source: AppliedAudioSource,
        allow_playing: bool,
    },
    Prepared {
        intent: SeekIntent,
        audio_revision: u64,
    },
}

#[derive(Clone, Copy)]
enum StartSource {
    Applied(AppliedAudioSource),
    Recovery(u64),
}

pub(super) struct PendingStart {
    generation: u64,
    revision: u64,
    source: StartSource,
    note_chase: Option<NoteChase>,
    start: Start,
}

pub(super) struct State {
    pub path: PathBuf,
    pub preferences: PlaybackPreferences,
    pub loading: bool,
    pub saving: bool,
    pub writable: bool,
    pub error: Option<String>,
    pub pending: Option<PendingStart>,
}

impl State {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            preferences: PlaybackPreferences::default(),
            loading: true,
            saving: false,
            writable: true,
            error: None,
            pending: None,
        }
    }

    fn policy(&self) -> NoteChase {
        if self.preferences.note_chase {
            NoteChase::On
        } else {
            NoteChase::Off
        }
    }
}

impl EditorView {
    pub(super) fn queue_normal_play(&mut self, frame: u64) {
        self.queue_normal_start(Start::Play { frame });
    }

    pub(super) fn queue_normal_start(&mut self, start: Start) {
        self.playback_settings.pending = None;
        let source = match &start {
            Start::Source { source, .. } => Some(StartSource::Applied(*source)),
            Start::Play { .. } if self.recovery.generation().is_some() => {
                self.recovery.generation().map(StartSource::Recovery)
            }
            _ => self.normal_start_source().map(StartSource::Applied),
        };
        let source = source.or_else(|| {
            if let Start::Play { frame } = &start
                && self.has_degraded_vst3_output()
            {
                // Open stopped recovery only. Raw auto-Play must not race the settings load.
                self.recover_audio_at(*frame);
                self.recovery.generation().map(StartSource::Recovery)
            } else {
                None
            }
        });
        let Some(source) = source else {
            self.error =
                Some("playback source is unavailable · retry playback when audio is ready".into());
            return;
        };
        let pending = PendingStart {
            generation: self.project_generation,
            revision: self.state.as_ref().map_or(0, |state| state.revision),
            source,
            note_chase: (!self.playback_settings.loading).then(|| self.playback_settings.policy()),
            start,
        };
        self.deferred_play = None;
        if self.playback_settings.loading {
            if matches!(pending.source, StartSource::Applied(_)) {
                self.cancel_section_launch_wait();
                self.invalidate_prepared_seek();
            }
            self.playback_settings.pending = Some(pending);
            self.status = "loading playback settings · playback starts when ready".into();
            self.wake_poll();
        } else {
            self.dispatch_normal_start(pending);
        }
    }

    fn normal_start_source(&self) -> Option<AppliedAudioSource> {
        if self.recovery.generation().is_some()
            || self.stopped_playback_restore.is_some()
            || !self.pending_audio_commands.is_empty()
        {
            return None;
        }
        let audio = self.audio.as_ref()?;
        let status = audio.control.status()?;
        if status.stopped_edit_pending()
            || status
                .device_fault
                .load(std::sync::atomic::Ordering::Acquire)
            || status
                .render_fault
                .load(std::sync::atomic::Ordering::Acquire)
            || status
                .shutdown_complete
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        let source = status.applied_audio_source()?;
        (audio.control.output_identity() == Some(source.output_identity)
            && source.context.project_generation == self.project_generation
            && self.state.as_ref().is_some_and(|state| {
                state.audio_authoring_revision(source.context) == Some(state.revision)
            }))
        .then_some(source)
    }

    fn dispatch_normal_start(&mut self, mut pending: PendingStart) {
        let current = pending.generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == pending.revision)
            && !self.opening_project
            && !self.close_prompt_open;
        if !current {
            self.error =
                Some("playback target changed while settings loaded · retry playback".into());
            return;
        }
        let note_chase = *pending
            .note_chase
            .get_or_insert_with(|| self.playback_settings.policy());
        let source = match pending.source {
            StartSource::Applied(source) if self.normal_start_source() == Some(source) => source,
            StartSource::Recovery(generation) if self.recovery.generation() == Some(generation) => {
                self.playback_settings.pending = Some(pending);
                self.status = "restarting audio output · playback starts when ready".into();
                return;
            }
            _ => {
                self.error =
                    Some("playback source changed while settings loaded · retry playback".into());
                return;
            }
        };
        match pending.start {
            Start::Session { command, revision } => {
                self.queue_session_launch_with_note_chase(command, revision, note_chase);
            }
            Start::Play { frame } => {
                self.queue_applied_source_play_with_note_chase(frame, source, false, note_chase);
            }
            Start::Source {
                frame,
                source,
                allow_playing,
            } => {
                self.queue_applied_source_play_with_note_chase(
                    frame,
                    source,
                    allow_playing,
                    note_chase,
                );
            }
            Start::Prepared {
                mut intent,
                audio_revision,
            } => {
                intent.note_chase = note_chase;
                if matches!(
                    intent.kind,
                    super::arrangement_seek::SeekKind::CommittedLocator { .. }
                ) {
                    self.admit_prepared_seek(intent, audio_revision);
                } else {
                    self.submit_prepared_seek_intent(intent, audio_revision);
                }
            }
        }
    }

    /// The existing recovery owner transfers only its exact completed generation's witness.
    pub(super) fn normal_start_recovery_completed(&mut self, generation: u64) {
        let Some(mut pending) = self.playback_settings.pending.take() else {
            return;
        };
        match pending.source {
            StartSource::Recovery(expected) if expected == generation => {}
            StartSource::Recovery(_) => {
                self.error = Some("playback recovery changed · retry playback".into());
                return;
            }
            StartSource::Applied(_) => {
                self.playback_settings.pending = Some(pending);
                return;
            }
        }
        let Some(source) = self.normal_start_source() else {
            self.error = Some("recovered playback source is unavailable · retry playback".into());
            return;
        };
        pending.source = StartSource::Applied(source);
        if self.playback_settings.loading {
            self.playback_settings.pending = Some(pending);
        } else {
            self.dispatch_normal_start(pending);
        }
    }

    pub(super) fn start_playback_preferences_load(&mut self, cx: &mut Context<Self>) {
        let path = self.playback_settings.path.clone();
        #[cfg(test)]
        let environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _environment = environment;
                    hex_preferences::load_playback_preferences(&path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.playback_settings.loading = false;
                match result {
                    Ok(preferences) => {
                        view.playback_settings.preferences = preferences;
                        if let Some(pending) = view.playback_settings.pending.take() {
                            view.dispatch_normal_start(pending);
                        }
                    }
                    Err(error) => {
                        view.playback_settings.writable = false;
                        view.playback_settings.pending = None;
                        let error = format!(
                            "could not load playback settings · {}",
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string()
                            )
                        );
                        view.playback_settings.error = Some(error.clone());
                        view.error = Some(error.into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Serialize an explicit preference edit; the active policy changes only on success.
    #[allow(
        dead_code,
        reason = "settings publication is independent of menu composition"
    )]
    pub(super) fn save_note_chase(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let state = &mut self.playback_settings;
        if state.loading || state.saving || !state.writable {
            return;
        }
        let preferences = PlaybackPreferences {
            note_chase: enabled,
        };
        state.saving = true;
        state.error = None;
        let path = state.path.clone();
        #[cfg(test)]
        let environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _environment = environment;
                    hex_preferences::save_playback_preferences(&path, preferences)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.playback_settings.saving = false;
                match result {
                    Ok(()) => view.playback_settings.preferences = preferences,
                    Err(error) => {
                        let error = format!(
                            "could not save playback settings · {}",
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string()
                            )
                        );
                        view.playback_settings.error = Some(error.clone());
                        view.error = Some(error.into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
