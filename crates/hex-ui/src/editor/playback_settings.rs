//! Application playback policy and one bounded normal-start slot during startup.
//! Raw Cue/Seek, history and structural resume retain their independent laws.
//! The UI captures targets before waiting; late loading cannot retarget a start.
use super::{EditorView, arrangement_seek::SeekIntent};
use gpui::Context;
use hex_audio::prepared::AppliedAudioSource;
use hex_engine::{NoteChase, PlaybackAction, PlaybackCommand};
use hex_preferences::PlaybackPreferences;
use std::path::PathBuf;

pub(super) enum Start {
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

pub(super) struct PendingStart {
    generation: u64,
    revision: u64,
    source: Option<AppliedAudioSource>,
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
        let source = match &start {
            Start::Source { source, .. } => Some(*source),
            _ => self.normal_start_source(),
        };
        let pending = PendingStart {
            generation: self.project_generation,
            revision: self.state.as_ref().map_or(0, |state| state.revision),
            source,
            start,
        };
        self.playback_settings.pending = None;
        if self.playback_settings.loading {
            self.cancel_section_launch_wait();
            self.invalidate_prepared_seek();
            self.playback_settings.pending = Some(pending);
            self.status = "loading playback settings · playback starts when ready".into();
            self.wake_poll();
        } else {
            self.dispatch_normal_start(pending, self.playback_settings.policy());
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

    fn dispatch_normal_start(&mut self, pending: PendingStart, note_chase: NoteChase) {
        if pending.generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != pending.revision)
            || self.opening_project
            || self.close_prompt_open
            || pending
                .source
                .is_some_and(|source| self.normal_start_source() != Some(source))
        {
            self.error =
                Some("playback target changed while settings loaded · retry playback".into());
            return;
        }
        match pending.start {
            Start::Play { frame } => {
                let command = if note_chase == NoteChase::On {
                    PlaybackCommand::PlayFrom(frame)
                } else {
                    PlaybackCommand::PlayFromWithNoteChase { frame, note_chase }
                };
                if self.recovery.generation().is_some() || self.recover_degraded_play_at(frame) {
                    self.deferred_play = Some((
                        PlaybackAction::Transport(command),
                        None,
                        self.project_generation,
                    ));
                    self.status = "restarting audio output · playback starts when ready".into();
                } else if let Some(source) = pending.source {
                    self.queue_applied_source_play_with_note_chase(
                        frame, source, false, note_chase,
                    );
                } else {
                    self.queue_audio_command(command);
                }
                #[cfg(test)]
                {
                    self.last_transport_command = Some(command);
                }
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
                            view.dispatch_normal_start(pending, view.playback_settings.policy());
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
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
