use crate::{Command, Created, Invalid, Project};
use std::collections::VecDeque;
use std::sync::Arc;

/// Bounded snapshot history on the control thread. Oldest edits expire at capacity.
/// IDs never rewind across undo, redo or branching, so stale references stay stale.
struct HistoryEntry {
    project: Project,
    midi_effect: Option<crate::MidiHistoryEffect>,
    // Structural intent can affect only markers in an empty arrangement.
    // Snapshot audio differences cannot recover the required publication law.
    section_publication: bool,
    // Ephemeral history-edge custody, never saved project state. Both directions
    // replace baked/live source ownership under the stopped Arrangement gate.
    stopped_bounce: bool,
    // A staged plugin gesture must use the same receipt-bound publication for
    // both directions of its history edge.
    plugin_publication: bool,
}

pub struct Editor {
    project: Project,
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    capacity: usize,
    pending_plugin_state: Option<Arc<()>>,
}

/// Validated, reserved plugin state edit that has not changed project history.
/// Its holder must independently correlate an audio publication terminal before
/// committing. Other canonical mutations invalidate the token.
pub struct PreparedPluginState {
    identity: Arc<()>,
    candidate: Project,
    kind: PreparedPluginKind,
}
enum PreparedPluginKind {
    Edit(Created),
    History {
        redo: bool,
        midi_effect: Option<crate::MidiHistoryEffect>,
        section_publication: bool,
        stopped_bounce: bool,
        plugin_publication: bool,
    },
}
impl PreparedPluginState {
    pub fn project(&self) -> &Project {
        &self.candidate
    }
}
impl Editor {
    pub fn new(project: Project, capacity: usize) -> Result<Self, Invalid> {
        if !(1..=128).contains(&capacity) {
            return Err(Invalid("history capacity must be 1..128"));
        }
        Ok(Self {
            project,
            undo: VecDeque::new(),
            redo: Vec::new(),
            capacity,
            pending_plugin_state: None,
        })
    }
    pub fn project(&self) -> &Project {
        &self.project
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// Whether the adjacent history edge originated in a staged plugin edit.
    pub fn next_history_requires_plugin_publication(&self, redo: bool) -> bool {
        (if redo {
            self.redo.last()
        } else {
            self.undo.back()
        })
        .is_some_and(|edge| edge.plugin_publication)
    }
    /// Inspect the actual next global history edge without cloning or consuming it.
    /// The target is borrowed before ID and recording-epoch normalization; callers
    /// must classify again inside the prepared transition before committing.
    pub fn inspect_history<T>(
        &self,
        redo: bool,
        inspect: impl FnOnce(&Project, &Project) -> T,
    ) -> Option<T> {
        let target = if redo {
            self.redo.last()
        } else {
            self.undo.back()
        }?;
        Some(inspect(&self.project, &target.project))
    }

    /// In-place bake edges replace live/baked voice custody in both directions.
    /// This control-only history annotation is not serialized with the project.
    pub fn next_history_requires_stopped_bounce(&self, redo: bool) -> bool {
        (if redo {
            self.redo.last()
        } else {
            self.undo.back()
        })
        .is_some_and(|edge| edge.stopped_bounce)
    }

    /// Origin of the actual adjacent edge, minted only by canonical single-command execution.
    pub fn next_midi_history_effect(&self, redo: bool) -> Option<crate::MidiHistoryEffect> {
        if redo {
            self.redo.last()
        } else {
            self.undo.back()
        }?
        .midi_effect
    }

    /// Origin of the actual adjacent structural section edge. The same flag
    /// follows ordinary/prepared Undo and Redo; refusals and no-ops retain it.
    pub fn next_section_history_requires_publication(&self, redo: bool) -> bool {
        if redo {
            self.redo.last()
        } else {
            self.undo.back()
        }
        .is_some_and(|entry| entry.section_publication)
    }

    pub fn execute(&mut self, command: Command) -> Result<Created, Invalid> {
        self.execute_validated(command, |_| Ok(()))
    }

    /// Prepare only a plugin state replacement; no project or history mutation
    /// occurs. The caller owns the token and must fence competing publications.
    pub fn prepare_plugin_state(
        &mut self,
        device: crate::DeviceId,
        state: hex_lattice_contract::NativeDeviceState,
    ) -> Result<PreparedPluginState, Invalid> {
        if state.type_id() != hex_lattice_contract::DeviceTypeId::from_static("hex.vst3")
            || self
                .project
                .device(device)
                .is_none_or(|entry| entry.state().type_id() != state.type_id())
        {
            return Err(Invalid("plugin state target differs"));
        }
        let mut candidate = self.project.clone();
        let created = candidate.apply(Command::ReplaceDeviceState { device, state })?;
        candidate.validate_source_bindings()?;
        candidate.validate_track_hierarchy()?;
        candidate.validate_changed_midi_occupancy(&self.project)?;
        candidate.validate_changed_arrangement_occupancy(&self.project)?;
        candidate.validate_retained_audio_roots()?;
        candidate.validate_device_asset_references()?;
        candidate.validate_device_automation()?;
        candidate.validate_midi_volumes()?;
        candidate.validate_playable_routes()?;
        if candidate != self.project {
            self.undo
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve edit history"))?;
        }
        let identity = Arc::new(());
        self.pending_plugin_state = Some(Arc::clone(&identity));
        Ok(PreparedPluginState {
            identity,
            candidate,
            kind: PreparedPluginKind::Edit(created),
        })
    }

    /// Reserve the exact adjacent history edge for a plugin publication. The
    /// source and both stacks stay unchanged until the caller commits its token.
    pub fn prepare_plugin_history(
        &mut self,
        redo: bool,
    ) -> Result<Option<PreparedPluginState>, Invalid> {
        let edge = if redo {
            self.redo.last()
        } else {
            self.undo.back()
        };
        let Some(edge) = edge else {
            return Ok(None);
        };
        let mut candidate = edge.project.clone();
        let kind = PreparedPluginKind::History {
            redo,
            midi_effect: edge.midi_effect,
            section_publication: edge.section_publication,
            stopped_bounce: edge.stopped_bounce,
            plugin_publication: edge.plugin_publication,
        };
        candidate.next_id = candidate.next_id.max(self.project.next_id);
        candidate.retain_gain_touch_epochs_from(&self.project);
        if redo {
            self.undo
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve undo history"))?;
        } else {
            self.redo
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve redo history"))?;
        }
        let identity = Arc::new(());
        self.pending_plugin_state = Some(Arc::clone(&identity));
        Ok(Some(PreparedPluginState {
            identity,
            candidate,
            kind,
        }))
    }

    /// Install a still-current prepared state after its independently verified
    /// publication terminal. Refused or stale tokens never change history.
    pub fn commit_plugin_state(
        &mut self,
        prepared: PreparedPluginState,
    ) -> Result<Created, Invalid> {
        self.try_commit_plugin_state(prepared)
            .map_err(|(_, error)| error)
    }

    /// Retain the original candidate on refusal for control-side recovery when
    /// the audio graph may already have been installed.
    #[allow(clippy::result_large_err)]
    pub fn try_commit_plugin_state(
        &mut self,
        prepared: PreparedPluginState,
    ) -> Result<Created, (PreparedPluginState, Invalid)> {
        if !self.plugin_state_is_current(&prepared) {
            return Err((prepared, Invalid("prepared plugin state is stale")));
        }
        if let PreparedPluginKind::History { redo, .. } = &prepared.kind
            && if *redo {
                self.redo.is_empty()
            } else {
                self.undo.is_empty()
            }
        {
            return Err((prepared, Invalid("prepared history edge is unavailable")));
        }
        self.pending_plugin_state = None;
        match prepared.kind {
            PreparedPluginKind::Edit(created) => {
                if prepared.candidate != self.project {
                    let previous = std::mem::replace(&mut self.project, prepared.candidate);
                    if self.undo.len() == self.capacity {
                        self.undo.pop_front();
                    }
                    self.undo.push_back(HistoryEntry {
                        project: previous,
                        midi_effect: None,
                        section_publication: false,
                        stopped_bounce: false,
                        plugin_publication: true,
                    });
                    self.redo.clear();
                }
                Ok(created)
            }
            PreparedPluginKind::History {
                redo,
                midi_effect,
                section_publication,
                stopped_bounce,
                plugin_publication,
            } => {
                let removed = if redo {
                    self.redo.pop()
                } else {
                    self.undo.pop_back()
                };
                // The exact edge was checked before consuming the token; no
                // external mutation can interleave on this editor borrow.
                drop(removed);
                let previous = std::mem::replace(&mut self.project, prepared.candidate);
                let entry = HistoryEntry {
                    project: previous,
                    midi_effect,
                    section_publication,
                    stopped_bounce,
                    plugin_publication,
                };
                if redo {
                    if self.undo.len() == self.capacity {
                        self.undo.pop_front();
                    }
                    self.undo.push_back(entry);
                } else {
                    self.redo.push(entry);
                }
                Ok(Created::None)
            }
        }
    }

    /// Whether this exact reservation still belongs to the current editor edge.
    /// This observation grants no audio publication or commit authority.
    pub fn plugin_state_is_current(&self, prepared: &PreparedPluginState) -> bool {
        self.pending_plugin_state
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(pending, &prepared.identity))
    }

    /// Retire the matching token without changing the canonical project.
    pub fn cancel_plugin_state(&mut self, prepared: PreparedPluginState) -> bool {
        if self
            .pending_plugin_state
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(pending, &prepared.identity))
        {
            self.pending_plugin_state = None;
            true
        } else {
            false
        }
    }

    /// Validate a prepared candidate before committing it or changing history.
    /// Control-side callers can reject edits that cannot produce a playable
    /// graph without consuming IDs, clearing redo, or introducing an undo step.
    pub fn execute_validated<E: From<Invalid>>(
        &mut self,
        command: Command,
        validate: impl FnOnce(&Project) -> Result<(), E>,
    ) -> Result<Created, E> {
        self.execute_prepared(command, validate)
            .map(|(created, ())| created)
    }

    /// Prepare a control-side artifact from the candidate and return that exact
    /// artifact with the committed edit. Failure preserves project and history.
    pub fn execute_prepared<T, E: From<Invalid>>(
        &mut self,
        command: Command,
        prepare: impl FnOnce(&Project) -> Result<T, E>,
    ) -> Result<(Created, T), E> {
        self.execute_prepared_transition(command, |_, candidate| prepare(candidate))
    }
    /// Prepare against the exact borrowed source and candidate before one atomic
    /// history commit. No caller clone is needed to prove a tuning-only delta.
    pub fn execute_prepared_transition<T, E: From<Invalid>>(
        &mut self,
        command: Command,
        prepare: impl FnOnce(&Project, &Project) -> Result<T, E>,
    ) -> Result<(Created, T), E> {
        self.execute_transition(command, false, prepare)
    }
    /// Closed MIDI edit: refuse unsupported or out-of-scope effects before the
    /// preparation hook can seal a native candidate.
    pub fn execute_midi_prepared_transition<T, E: From<Invalid>>(
        &mut self,
        command: Command,
        prepare: impl FnOnce(&Project, &Project) -> Result<T, E>,
    ) -> Result<(Created, T), E> {
        self.execute_transition(command, true, prepare)
    }
    fn execute_transition<T, E: From<Invalid>>(
        &mut self,
        command: Command,
        require_midi: bool,
        prepare: impl FnOnce(&Project, &Project) -> Result<T, E>,
    ) -> Result<(Created, T), E> {
        let mut command = command;
        let alias_plan = crate::clip_alias_edit::Plan::capture(&self.project, &mut command)?;
        let section_publication = command.requires_section_publication();
        let stopped_bounce = matches!(
            &command,
            Command::CommitArrangementBounce { target, in_place, .. } if target.bakes_devices(*in_place)
        );
        let plan = crate::midi_history::Plan::capture(&command, &self.project);
        let plan = if require_midi {
            plan?
        } else {
            plan.ok().flatten()
        };
        if require_midi && plan.is_none() {
            return Err(E::from(Invalid("unsupported midi edit command")));
        }
        let mut candidate = self.project.clone();
        let result = candidate.apply(command)?;
        let alias_changed = alias_plan.finish(&mut candidate)?;
        if alias_changed {
            Project::from_snapshot(candidate.snapshot())?;
        }
        candidate.validate_source_bindings()?;
        candidate.validate_track_hierarchy()?;
        candidate.validate_changed_midi_occupancy(&self.project)?;
        candidate.validate_changed_arrangement_occupancy(&self.project)?;
        candidate.validate_retained_audio_roots()?;
        candidate.validate_device_asset_references()?;
        candidate.validate_device_automation()?;
        candidate.validate_midi_volumes()?;
        candidate.validate_playable_routes()?;
        let effect = plan.and_then(|p| p.finish(result, &self.project, &candidate));
        if require_midi && candidate != self.project && effect.is_none() {
            return Err(E::from(Invalid(
                "midi command changed unsupported project state",
            )));
        }
        if candidate != self.project {
            self.undo
                .try_reserve(1)
                .map_err(|_| E::from(Invalid("could not reserve edit history")))?;
        }
        let prepared = prepare(&self.project, &candidate)?;
        // No-op controls should neither add history nor erase a redo branch.
        if candidate == self.project {
            return Ok((result, prepared));
        }
        self.pending_plugin_state = None;
        let previous = std::mem::replace(&mut self.project, candidate);
        if self.undo.len() == self.capacity {
            self.undo.pop_front();
        }
        self.undo.push_back(HistoryEntry {
            project: previous,
            midi_effect: effect,
            section_publication,
            stopped_bounce,
            plugin_publication: false,
        });
        self.redo.clear();
        Ok((result, prepared))
    }
    /// Apply at most MAX_CLIPS ordered commands to one private candidate and
    /// prepare it once. Preparation sees each command's result in input order.
    /// Validate the final routing graph, not each intermediate batch state.
    /// Any refusal preserves IDs, both history stacks and the current project.
    pub fn execute_many_prepared<T, E: From<Invalid>>(
        &mut self,
        commands: Vec<Command>,
        prepare: impl FnOnce(&Project, &[Created]) -> Result<T, E>,
    ) -> Result<(Vec<Created>, T), E> {
        self.execute_batch_prepared(commands, None, prepare)
    }

    /// Import an Arrangement audio batch as one replacement plan. Console imports
    /// retain their ordered cell commands. Only the final candidate is prepared;
    /// intermediate split capacity cannot reject an otherwise admissible batch.
    pub fn execute_audio_imports_prepared<T, E: From<Invalid>>(
        &mut self,
        commands: Vec<Command>,
        prepare: impl FnOnce(&Project, &[Created]) -> Result<T, E>,
    ) -> Result<(Vec<Created>, T), E> {
        self.execute_audio_imports_with_warp_prepared(
            commands,
            crate::AudioImportWarp::Off,
            prepare,
        )
    }

    /// Apply the captured import policy to each new Arrangement source before
    /// placement and preparation. Refusal preserves the complete previous history.
    /// Console imports remain unwarped and reject a non-Off policy.
    pub fn execute_audio_imports_with_warp_prepared<T, E: From<Invalid>>(
        &mut self,
        commands: Vec<Command>,
        warp: crate::AudioImportWarp,
        prepare: impl FnOnce(&Project, &[Created]) -> Result<T, E>,
    ) -> Result<(Vec<Created>, T), E> {
        let arrangement = !commands.is_empty()
            && commands.iter().all(|command| {
                matches!(
                    command,
                    Command::ImportPreparedAudioTrackAt { .. }
                        | Command::ImportPreparedAudioClip { .. }
                        | Command::ImportPreparedAudioClipWithOverwrite { .. }
                )
            });
        if !arrangement && warp != crate::AudioImportWarp::Off {
            return Err(E::from(Invalid(
                "auto-warp requires arrangement audio imports",
            )));
        }
        self.execute_batch_prepared(commands, arrangement.then_some(warp), prepare)
    }

    fn execute_batch_prepared<T, E: From<Invalid>>(
        &mut self,
        commands: Vec<Command>,
        arrangement_import: Option<crate::AudioImportWarp>,
        prepare: impl FnOnce(&Project, &[Created]) -> Result<T, E>,
    ) -> Result<(Vec<Created>, T), E> {
        if commands.len() > crate::MAX_CLIPS {
            return Err(E::from(Invalid("batch command limit reached")));
        }
        let section_publication = commands.iter().any(Command::requires_section_publication);
        let mut results = Vec::new();
        results
            .try_reserve_exact(commands.len())
            .map_err(|_| E::from(Invalid("could not reserve batch results")))?;
        let mut candidate = self.project.clone();
        if let Some(warp) = arrangement_import {
            results = candidate.apply_audio_imports(commands, warp)?;
        } else {
            let mut alias_changed = false;
            for mut command in commands {
                let alias_plan = crate::clip_alias_edit::Plan::capture(&candidate, &mut command)?;
                results.push(candidate.apply(command)?);
                alias_changed |= alias_plan.finish(&mut candidate)?;
            }
            if alias_changed {
                Project::from_snapshot(candidate.snapshot())?;
            }
        }
        candidate.validate_track_hierarchy()?;
        candidate.validate_changed_midi_occupancy(&self.project)?;
        candidate.validate_changed_arrangement_occupancy(&self.project)?;
        candidate.validate_retained_audio_roots()?;
        candidate.validate_device_asset_references()?;
        candidate.validate_device_automation()?;
        candidate.validate_midi_volumes()?;
        candidate.validate_playable_routes()?;
        // Reserve before preparation/commit. No history allocation follows it.
        self.undo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve batch history")))?;
        let prepared = prepare(&candidate, &results)?;
        if candidate != self.project {
            self.pending_plugin_state = None;
            let previous = std::mem::replace(&mut self.project, candidate);
            if self.undo.len() == self.capacity {
                self.undo.pop_front();
            }
            self.undo.push_back(HistoryEntry {
                project: previous,
                midi_effect: None,
                section_publication,
                stopped_bounce: false,
                plugin_publication: false,
            });
            self.redo.clear();
        }
        Ok((results, prepared))
    }

    pub fn undo(&mut self) -> bool {
        let Some(mut previous) = self.undo.pop_back() else {
            return false;
        };
        self.pending_plugin_state = None;
        previous.project.next_id = previous.project.next_id.max(self.project.next_id);
        previous
            .project
            .retain_gain_touch_epochs_from(&self.project);
        self.redo.push(HistoryEntry {
            project: std::mem::replace(&mut self.project, previous.project),
            midi_effect: previous.midi_effect,
            section_publication: previous.section_publication,
            stopped_bounce: previous.stopped_bounce,
            plugin_publication: previous.plugin_publication,
        });
        true
    }

    /// Prepare the previous snapshot before moving it across history. Failure
    /// leaves the project, allocator high-water mark, and both history stacks
    /// unchanged.
    pub fn undo_prepared<T, E: From<Invalid>>(
        &mut self,
        prepare: impl FnOnce(&Project) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        self.undo_prepared_transition(|_, candidate| prepare(candidate))
    }
    /// Borrow source and target before changing either history stack.
    pub fn undo_prepared_transition<T, E: From<Invalid>>(
        &mut self,
        prepare: impl FnOnce(&Project, &Project) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        let Some(previous) = self.undo.back() else {
            return Ok(None);
        };
        let mut candidate = previous.project.clone();
        let effect = previous.midi_effect;
        let section_publication = previous.section_publication;
        let stopped_bounce = previous.stopped_bounce;
        let plugin_publication = previous.plugin_publication;
        candidate.next_id = candidate.next_id.max(self.project.next_id);
        candidate.retain_gain_touch_epochs_from(&self.project);
        self.redo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve redo history")))?;
        let prepared = prepare(&self.project, &candidate)?;
        let Some(_) = self.undo.pop_back() else {
            return Err(E::from(Invalid("undo history changed during preparation")));
        };
        self.pending_plugin_state = None;
        self.redo.push(HistoryEntry {
            project: std::mem::replace(&mut self.project, candidate),
            midi_effect: effect,
            section_publication,
            stopped_bounce,
            plugin_publication,
        });
        Ok(Some(prepared))
    }

    pub fn redo(&mut self) -> bool {
        let Some(mut next) = self.redo.pop() else {
            return false;
        };
        self.pending_plugin_state = None;
        next.project.next_id = next.project.next_id.max(self.project.next_id);
        next.project.retain_gain_touch_epochs_from(&self.project);
        self.undo.push_back(HistoryEntry {
            project: std::mem::replace(&mut self.project, next.project),
            midi_effect: next.midi_effect,
            section_publication: next.section_publication,
            stopped_bounce: next.stopped_bounce,
            plugin_publication: next.plugin_publication,
        });
        true
    }

    /// Prepare the next snapshot before moving it across history. Failure
    /// leaves the project, allocator high-water mark, and both history stacks
    /// unchanged.
    pub fn redo_prepared<T, E: From<Invalid>>(
        &mut self,
        prepare: impl FnOnce(&Project) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        self.redo_prepared_transition(|_, candidate| prepare(candidate))
    }
    /// Borrow source and target before changing either history stack.
    pub fn redo_prepared_transition<T, E: From<Invalid>>(
        &mut self,
        prepare: impl FnOnce(&Project, &Project) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        let Some(next) = self.redo.last() else {
            return Ok(None);
        };
        let mut candidate = next.project.clone();
        let effect = next.midi_effect;
        let section_publication = next.section_publication;
        let stopped_bounce = next.stopped_bounce;
        let plugin_publication = next.plugin_publication;
        candidate.next_id = candidate.next_id.max(self.project.next_id);
        candidate.retain_gain_touch_epochs_from(&self.project);
        self.undo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve undo history")))?;
        let prepared = prepare(&self.project, &candidate)?;
        let Some(_) = self.redo.pop() else {
            return Err(E::from(Invalid("redo history changed during preparation")));
        };
        self.pending_plugin_state = None;
        if self.undo.len() == self.capacity {
            self.undo.pop_front();
        }
        self.undo.push_back(HistoryEntry {
            project: std::mem::replace(&mut self.project, candidate),
            midi_effect: effect,
            section_publication,
            stopped_bounce,
            plugin_publication,
        });
        Ok(Some(prepared))
    }
}

#[cfg(test)]
mod plugin_state_transaction_tests {
    use super::*;
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    fn state(value: u8) -> NativeDeviceState {
        NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![value]).unwrap()
    }
    fn fixture() -> (Editor, crate::DeviceId) {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: state(1),
            })
            .unwrap()
        else {
            panic!("device")
        };
        (editor, device)
    }
    #[test]
    fn staged_plugin_state_cancel_and_commit_preserve_one_history_edge() {
        let (mut editor, device) = fixture();
        let original = editor.project().clone();
        assert!(!editor.next_history_requires_plugin_publication(false));
        let cancelled = editor.prepare_plugin_state(device, state(2)).unwrap();
        assert_eq!(editor.project(), &original);
        assert_eq!(
            cancelled.project().device(device).unwrap().state(),
            &state(2)
        );
        assert!(editor.cancel_plugin_state(cancelled));
        assert_eq!(editor.project(), &original);
        let token = editor.prepare_plugin_state(device, state(3)).unwrap();
        assert_eq!(editor.project(), &original);
        assert_eq!(editor.commit_plugin_state(token).unwrap(), Created::None);
        assert!(editor.next_history_requires_plugin_publication(false));
        assert_eq!(editor.project().device(device).unwrap().state(), &state(3));
        let refused_undo = editor.prepare_plugin_history(false).unwrap().unwrap();
        assert_eq!(refused_undo.project(), &original);
        assert_eq!(editor.project().device(device).unwrap().state(), &state(3));
        assert!(editor.cancel_plugin_state(refused_undo));
        assert!(editor.next_history_requires_plugin_publication(false));
        assert_eq!(editor.project().device(device).unwrap().state(), &state(3));
        let undo = editor.prepare_plugin_history(false).unwrap().unwrap();
        editor.commit_plugin_state(undo).unwrap();
        assert!(editor.next_history_requires_plugin_publication(true));
        assert_eq!(editor.project(), &original);
        let redo = editor.prepare_plugin_history(true).unwrap().unwrap();
        assert_eq!(editor.project(), &original);
        editor.commit_plugin_state(redo).unwrap();
        assert!(editor.next_history_requires_plugin_publication(false));
        assert_eq!(editor.project().device(device).unwrap().state(), &state(3));
        assert!(editor.undo());
        assert!(editor.next_history_requires_plugin_publication(true));
        assert!(editor.redo());
        assert!(editor.next_history_requires_plugin_publication(false));
        editor.undo_prepared::<(), Invalid>(|_| Ok(())).unwrap();
        assert!(editor.next_history_requires_plugin_publication(true));
        editor.redo_prepared::<(), Invalid>(|_| Ok(())).unwrap();
        assert!(editor.next_history_requires_plugin_publication(false));
    }
    #[test]
    fn stale_token_cannot_commit_after_aba_or_replace_another_pending_token() {
        let (mut editor, device) = fixture();
        let original = editor.project().clone();
        let stale = editor.prepare_plugin_state(device, state(2)).unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: state(3),
            })
            .unwrap();
        assert!(editor.undo());
        assert_eq!(editor.project(), &original);
        assert!(editor.commit_plugin_state(stale).is_err());
        assert_eq!(editor.project(), &original);
        let first = editor.prepare_plugin_state(device, state(4)).unwrap();
        let second = editor.prepare_plugin_state(device, state(5)).unwrap();
        assert!(!editor.cancel_plugin_state(first));
        assert_eq!(editor.project(), &original);
        editor.commit_plugin_state(second).unwrap();
        assert_eq!(editor.project().device(device).unwrap().state(), &state(5));
    }
}

#[cfg(test)]
mod gain_touch_epoch_tests {
    use super::*;
    #[test]
    fn ordinary_and_prepared_history_never_rewind_surviving_owner_epoch() {
        for prepared in [false, true] {
            let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
            let Created::Track(id) = editor
                .execute(Command::AddTrack {
                    name: "touch".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            editor
                .execute(Command::Gain {
                    track: id,
                    gain: 0.5,
                })
                .unwrap();
            // Private metadata fixture: no production recording/adoption API is opened.
            editor.project.tracks[0].gain_lane.recording_epoch = 9;
            if prepared {
                assert!(
                    editor
                        .undo_prepared::<_, Invalid>(|p| {
                            assert_eq!(p.track(id).unwrap().gain_lane().recording_epoch(), 9);
                            Ok(())
                        })
                        .unwrap()
                        .is_some()
                );
            } else {
                assert!(editor.undo());
            }
            assert_eq!(
                editor
                    .project
                    .track(id)
                    .unwrap()
                    .gain_lane()
                    .recording_epoch(),
                9
            );
            editor.project.tracks[0].gain_lane.recording_epoch = 10;
            if prepared {
                assert!(
                    editor
                        .redo_prepared::<_, Invalid>(|p| {
                            assert_eq!(p.track(id).unwrap().gain_lane().recording_epoch(), 10);
                            Ok(())
                        })
                        .unwrap()
                        .is_some()
                );
            } else {
                assert!(editor.redo());
            }
            assert_eq!(
                editor
                    .project
                    .track(id)
                    .unwrap()
                    .gain_lane()
                    .recording_epoch(),
                10
            );
        }
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    #[test]
    fn batch_prepares_once_and_one_undo_restores_all_commands() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut calls = 0;
        let (results, tracks) = editor
            .execute_many_prepared(
                vec![
                    Command::AddTrack {
                        name: "first".into(),
                    },
                    Command::AddTrack {
                        name: "second".into(),
                    },
                ],
                |candidate, results| {
                    calls += 1;
                    assert_eq!(results.len(), 2);
                    Ok::<_, Invalid>(candidate.tracks().len())
                },
            )
            .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(tracks, 2);
        assert!(matches!(results[0], Created::Track(_)));
        assert!(matches!(results[1], Created::Track(_)));
        assert!(editor.undo());
        assert!(editor.project().tracks().is_empty());
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project().tracks().len(), 2);
    }
    #[test]
    fn late_command_and_preparation_refusal_preserve_ids_and_redo() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(Command::AddTrack {
                name: "prior".into(),
            })
            .unwrap();
        assert!(editor.undo());
        let before = editor.project().clone();
        let mut called = false;
        assert!(
            editor
                .execute_many_prepared(
                    vec![
                        Command::AddTrack {
                            name: "valid".into()
                        },
                        Command::AddTrack {
                            name: String::new()
                        }
                    ],
                    |_, _| {
                        called = true;
                        Ok::<_, Invalid>(())
                    }
                )
                .is_err()
        );
        assert!(!called);
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
        assert!(!editor.can_undo());
        assert!(
            editor
                .execute_many_prepared(
                    vec![Command::AddTrack {
                        name: "valid".into()
                    }],
                    |_, _| Err::<(), _>(Invalid("preparation refused"))
                )
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
        assert!(!editor.can_undo());
    }
    #[test]
    fn empty_batch_preserves_redo_and_oversized_refuses_before_preparation() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(Command::AddTrack {
                name: "prior".into(),
            })
            .unwrap();
        editor.undo();
        let before = editor.project().clone();
        editor
            .execute_many_prepared(Vec::new(), |_, results| {
                assert!(results.is_empty());
                Ok::<_, Invalid>(())
            })
            .unwrap();
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
        assert!(
            editor
                .execute_many_prepared(
                    vec![Command::MasterGain { gain: 1.0 }; crate::MAX_CLIPS + 1],
                    |_, _| -> Result<(), Invalid> { panic!("must refuse before preparation") }
                )
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
    }
}

#[cfg(test)]
mod section_history_tests {
    use super::*;
    #[test]
    fn structural_origin_survives_batch_refusal_noop_and_both_history_paths() {
        for batch in [false, true] {
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::ArrangementSection(section) = editor
                .execute(Command::AddArrangementSection {
                    name: "one".into(),
                    color: crate::ElementColor::Blue,
                    start: crate::BeatTime::ZERO,
                    end: crate::BeatTime::from_quarters(2).unwrap(),
                })
                .unwrap()
            else {
                panic!("section")
            };
            assert!(!editor.next_section_history_requires_publication(false));
            let command = Command::DuplicateArrangementSection(section);
            if batch {
                editor
                    .execute_many_prepared(vec![command], |_, _| Ok::<(), Invalid>(()))
                    .unwrap();
            } else {
                editor.execute(command).unwrap();
            }
            assert!(editor.next_section_history_requires_publication(false));
            let before = editor.project().snapshot();
            assert!(
                editor
                    .undo_prepared(|_| Err::<(), Invalid>(Invalid("fixture publication refusal")))
                    .is_err()
            );
            assert_eq!(editor.project().snapshot(), before);
            assert!(editor.next_section_history_requires_publication(false));
            assert!(editor.undo());
            assert!(editor.next_section_history_requires_publication(true));
            editor
                .execute(Command::RenameArrangementSection {
                    section,
                    name: "one".into(),
                })
                .unwrap();
            assert!(
                editor.next_section_history_requires_publication(true),
                "no-op must preserve the real redo edge"
            );
            assert!(
                editor
                    .redo_prepared(|_| Ok::<(), Invalid>(()))
                    .unwrap()
                    .is_some()
            );
            assert!(editor.next_section_history_requires_publication(false));
            assert!(
                editor
                    .undo_prepared(|_| Ok::<(), Invalid>(()))
                    .unwrap()
                    .is_some()
            );
            assert!(editor.next_section_history_requires_publication(true));
            assert!(editor.redo());
            editor
                .execute(Command::RenameArrangementSection {
                    section,
                    name: "renamed".into(),
                })
                .unwrap();
            assert!(!editor.next_section_history_requires_publication(false));
            assert!(editor.undo());
            assert!(editor.next_section_history_requires_publication(false));
            assert!(!editor.next_section_history_requires_publication(true));
        }
    }
}
