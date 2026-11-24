//! Application playback policy and one bounded normal-start slot during startup.
//! Raw Cue/Seek, history and structural resume retain their independent laws.
//! The UI captures targets before waiting; late loading cannot retarget a start.
use super::{EditorView, arrangement_seek::SeekIntent};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_audio::prepared::AppliedAudioSource;
use hex_engine::NoteChase;
use hex_preferences::PlaybackPreferences;
use std::path::PathBuf;

pub(super) enum Start {
    Section {
        section: hex_session::arrangement_seek::SectionLaunch,
    },
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
    pub path: Option<PathBuf>,
    pub preferences: PlaybackPreferences,
    pub loading: bool,
    pub saving: bool,
    pub writable: bool,
    pub error: Option<String>,
    pub pending: Option<PendingStart>,
}

impl State {
    pub fn new(path: Option<PathBuf>) -> Self {
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
        self.deferred_toggle_play = None;
        self.deferred_metadata_seek = None;
        let source = match &start {
            Start::Source { source, .. } => self
                .start_source_is_current(&start, *source)
                .then_some(StartSource::Applied(*source)),
            Start::Prepared {
                intent:
                    SeekIntent {
                        kind: super::arrangement_seek::SeekKind::CommittedLocator { source, .. },
                        ..
                    },
                ..
            } => self
                .committed_locator_seek_source_is_current(*source)
                .then_some(StartSource::Applied(*source)),
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
        if source.is_none()
            && let Start::Play { frame } = &start
        {
            if self.paused_republish_waiting_for_current() {
                self.tick_paused_republish();
            }
            if !self.playback_settings.loading && self.paused_republish_blocks_play() {
                let note_chase = self.playback_settings.policy();
                self.deferred_play = Some((
                    hex_engine::PlaybackAction::Transport(
                        hex_engine::PlaybackCommand::PlayFromWithNoteChase {
                            frame: *frame,
                            note_chase,
                        },
                    ),
                    None,
                    self.project_generation,
                ));
                self.status = "applying audio update · playback starts when ready".into();
                self.error = None;
                self.wake_poll();
                return;
            }
            if self.paused_republish_exhausted_for_current() {
                self.error = Some("stop playback to apply edit before playing".into());
                return;
            }
        }
        let Some(source) = source else {
            let message = if matches!(start, Start::Session { .. }) && self.audio.is_none() {
                "session playback requires an audio output"
            } else {
                "playback source is unavailable · retry playback when audio is ready"
            };
            self.error = Some(message.into());
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

    pub(super) fn normal_start_source(&self) -> Option<AppliedAudioSource> {
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
        // The fenced client uses this coherent ACK for ordinary and eligible
        // installs; applied_audio_source is only the constructor-specific ACK.
        let source = status.recording_source()?;
        (audio.control.output_identity() == Some(source.output_identity)
            && source.context.project_generation == self.project_generation
            && self.state.as_ref().is_some_and(|state| {
                state.audio_authoring_revision(source.context) == Some(state.revision)
            }))
        .then_some(source)
    }

    /// Supplied actions retain the existing receipt owner's exact installed-source proof,
    /// including a committed copy/locator view whose ordinary UI terminal is still pending.
    fn start_source_is_current(&self, start: &Start, source: AppliedAudioSource) -> bool {
        match start {
            Start::Source {
                source: expected, ..
            } => {
                *expected == source
                    && self.pending_audio_commands.is_empty()
                    && self.stopped_playback_restore.is_none()
                    && self.recovery.generation().is_none()
                    && (self.normal_start_source() == Some(source)
                        || self.section_copy_play_source() == Some(source)
                        || self.committed_locator_seek_source_is_current(source))
            }
            Start::Prepared { intent, .. } => match intent.kind {
                super::arrangement_seek::SeekKind::CommittedLocator {
                    source: expected, ..
                } => expected == source && self.committed_locator_seek_source_is_current(source),
                _ => self.normal_start_source() == Some(source),
            },
            _ => self.normal_start_source() == Some(source),
        }
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
            StartSource::Applied(source)
                if self.start_source_is_current(&pending.start, source) =>
            {
                source
            }
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
            Start::Section { section } => {
                self.prepare_section_launch_with_note_chase(section, note_chase);
            }
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
        let Some(path) = self.playback_settings.path.clone() else {
            self.playback_settings.loading = false;
            self.playback_settings.writable = false;
            self.playback_settings.error =
                Some("playback settings unavailable · home directory is unset".into());
            if let Some(pending) = self.playback_settings.pending.take() {
                self.dispatch_normal_start(pending);
            }
            return;
        };
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

    pub(super) fn live_mode_enabled(&self) -> bool {
        self.playback_settings.preferences.live_mode
    }

    /// Serialize an explicit preference edit; the active policy changes only on success.
    pub(super) fn save_note_chase(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let preferences = PlaybackPreferences {
            note_chase: enabled,
            ..self.playback_settings.preferences
        };
        self.save_playback_preferences_change(preferences, cx);
    }

    pub(super) fn toggle_live_mode(&mut self, cx: &mut Context<Self>) {
        let preferences = PlaybackPreferences {
            live_mode: !self.playback_settings.preferences.live_mode,
            ..self.playback_settings.preferences
        };
        self.save_playback_preferences_change(preferences, cx);
    }

    fn save_playback_preferences_change(
        &mut self,
        preferences: PlaybackPreferences,
        cx: &mut Context<Self>,
    ) {
        let state = &mut self.playback_settings;
        if state.loading || state.saving || !state.writable {
            return;
        }
        let Some(path) = state.path.clone() else {
            state.writable = false;
            state.error = Some("playback settings unavailable · home directory is unset".into());
            return;
        };
        state.saving = true;
        state.error = None;
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

    pub(super) fn live_mode_settings_row(&self, cx: &mut Context<Self>) -> Div {
        let state = &self.playback_settings;
        let style = super::device_panel::control_style();
        let enabled = !state.loading && !state.saving && state.writable;
        let label = if state.loading {
            "live mode · loading…"
        } else if state.saving {
            "live mode · saving…"
        } else if state.preferences.live_mode {
            "live mode · on"
        } else {
            "live mode · off (non-live mode)"
        };
        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .child(
                div()
                    .id("live mode settings")
                    .debug_selector(|| "live mode settings".into())
                    .text_xs()
                    .px(px(4.0))
                    .py(px(3.0))
                    .border_1()
                    .border_color(rgb(style.border))
                    .opacity(if enabled { 1.0 } else { 0.45 })
                    .when(enabled, |row| {
                        row.cursor_pointer()
                            .on_click(cx.listener(|view, _, _, cx| view.toggle_live_mode(cx)))
                    })
                    .child(theme::text(label)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(style.muted))
                    .child(theme::text(
                        "live mode asks for confirmation before edits that may audibly interrupt \
                         playback; non-live mode proceeds immediately and accepts audible glitches.",
                    )),
            );
        if let Some(error) = &state.error {
            row = row.child(
                div()
                    .id("playback settings status")
                    .debug_selector(|| "playback settings status".into())
                    .text_xs()
                    .child(theme::text(error)),
            );
        }
        row
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
