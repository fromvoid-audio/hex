use crate::{Command, Created, Invalid, Project};
use std::collections::VecDeque;

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
}

/// A rate-changing edge uses the exact same bounded history stacks as edits.
pub enum SampleRateHistoryAction {
    Apply {
        rate: u32,
        mode: crate::SampleRateMode,
    },
    Undo,
    Redo,
}

pub struct Editor {
    project: Project,
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    capacity: usize,
}
impl Editor {
    /// Start history for an already validated project, including retained source
    /// paths. Subsequent edits validate source and receiver bindings before commit.
    pub fn new(project: Project, capacity: usize) -> Result<Self, Invalid> {
        if !(1..=128).contains(&capacity) {
            return Err(Invalid("history capacity must be 1..128"));
        }
        Ok(Self {
            project,
            undo: VecDeque::new(),
            redo: Vec::new(),
            capacity,
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

    /// Prepare a rate edge with the optional live-state projection validated
    /// by the Session owner. The projection describes the outgoing project
    /// with its freshly captured device state. Failure and no-op restore the
    /// exact original model and leave both history stacks unchanged.
    pub fn change_sample_rate_prepared<T, E: From<Invalid>>(
        &mut self,
        action: SampleRateHistoryAction,
        outgoing: Option<Project>,
        prepare: impl FnOnce(&Project) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        if outgoing.as_ref().is_some_and(|project| {
            project.sample_rate() != self.project.sample_rate()
                || project.sample_rate_mode() != self.project.sample_rate_mode()
        }) {
            return Err(E::from(Invalid(
                "captured project clock differs from the current rate",
            )));
        }
        let original = outgoing.map(|project| std::mem::replace(&mut self.project, project));
        let result = match action {
            SampleRateHistoryAction::Apply { rate, mode } => {
                self.apply_sample_rate(rate, mode, prepare)
            }
            SampleRateHistoryAction::Undo => self.undo_prepared(prepare),
            SampleRateHistoryAction::Redo => self.redo_prepared(prepare),
        };
        if !matches!(result, Ok(Some(_)))
            && let Some(original) = original
        {
            self.project = original;
        }
        result
    }

    fn apply_sample_rate<T, E: From<Invalid>>(
        &mut self,
        rate: u32,
        mode: crate::SampleRateMode,
        prepare: impl FnOnce(&Project) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        let candidate = self
            .project
            .at_sample_rate(rate)?
            .with_sample_rate_mode(mode);
        if candidate == self.project {
            return Ok(None);
        }
        self.undo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve sample rate history")))?;
        let prepared = prepare(&candidate)?;
        self.commit_candidate(candidate, None, false, false);
        Ok(Some(prepared))
    }

    fn commit_candidate(
        &mut self,
        candidate: Project,
        midi_effect: Option<crate::MidiHistoryEffect>,
        section_publication: bool,
        stopped_bounce: bool,
    ) {
        let previous = std::mem::replace(&mut self.project, candidate);
        self.push_undo(previous, midi_effect, section_publication, stopped_bounce);
    }

    fn push_undo(
        &mut self,
        previous: Project,
        midi_effect: Option<crate::MidiHistoryEffect>,
        section_publication: bool,
        stopped_bounce: bool,
    ) {
        if self.undo.len() == self.capacity {
            self.undo.pop_front();
        }
        self.undo.push_back(HistoryEntry {
            project: previous,
            midi_effect,
            section_publication,
            stopped_bounce,
        });
        self.redo.clear();
    }

    /// Commit one device state with the actual pre-gesture state as its Undo
    /// preimage. Preparation still compares the canonical source and candidate.
    pub fn replace_captured_device_state<T, E: From<Invalid>>(
        &mut self,
        device: crate::DeviceId,
        before: hex_lattice_contract::NativeDeviceState,
        after: hex_lattice_contract::NativeDeviceState,
        prepare: impl FnOnce(&Project, &Project) -> Result<T, E>,
    ) -> Result<T, E> {
        let original = self
            .project
            .device(device)
            .ok_or(Invalid("device no longer exists"))?;
        if before.type_id() != original.state().type_id() || after.type_id() != before.type_id() {
            return Err(Invalid("captured device type changed").into());
        }
        let previous = self
            .project
            .preview_device_edit(&Command::ReplaceDeviceState {
                device,
                state: before,
            })?;
        let candidate = self
            .project
            .preview_device_edit(&Command::ReplaceDeviceState {
                device,
                state: after,
            })?;
        if previous == candidate {
            return Err(Invalid("captured device edit has no state change").into());
        }
        self.undo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve device edit history")))?;
        let prepared = prepare(&self.project, &candidate)?;
        self.project = candidate;
        self.push_undo(previous, None, false, false);
        Ok(prepared)
    }

    pub fn execute(&mut self, command: Command) -> Result<Created, Invalid> {
        self.execute_validated(command, |_| Ok(()))
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
        candidate.prune_deleted_sidechains();
        if alias_changed {
            Project::from_snapshot(candidate.snapshot())?;
        }
        candidate.validate_serial_units()?;
        candidate.validate_source_bindings()?;
        candidate.validate_track_hierarchy()?;
        candidate.validate_changed_midi_occupancy(&self.project)?;
        candidate.validate_changed_arrangement_occupancy(&self.project)?;
        candidate.validate_retained_audio_roots()?;
        candidate.validate_device_asset_references()?;
        candidate.validate_device_automation()?;
        candidate.validate_device_modulation()?;
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
        self.commit_candidate(candidate, effect, section_publication, stopped_bounce);
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

    /// Apply the captured import policy to each new or reused Arrangement source
    /// before placement and preparation. `Command::ImportExistingAudio` members
    /// place an existing asset under the same law without adding one. Refusal
    /// preserves the complete previous history.
    /// Console imports remain unwarped and reject a non-Off policy.
    pub fn execute_audio_imports_with_warp_prepared<T, E: From<Invalid>>(
        &mut self,
        commands: Vec<Command>,
        warp: crate::AudioImportWarp,
        prepare: impl FnOnce(&Project, &[Created]) -> Result<T, E>,
    ) -> Result<(Vec<Created>, T), E> {
        self.execute_audio_imports_with_track_gain_prepared(commands, warp, None, prepare)
    }

    /// Apply an optional linear gain to tracks created by an Arrangement import.
    /// Existing tracks and Console imports retain their authored gains. The gain
    /// is captured before preparation and participates in the same undo step.
    pub fn execute_audio_imports_with_track_gain_prepared<T, E: From<Invalid>>(
        &mut self,
        commands: Vec<Command>,
        warp: crate::AudioImportWarp,
        new_track_gain: Option<f32>,
        prepare: impl FnOnce(&Project, &[Created]) -> Result<T, E>,
    ) -> Result<(Vec<Created>, T), E> {
        if new_track_gain.is_some_and(|gain| !gain.is_finite() || !(0.0..=2.0).contains(&gain)) {
            return Err(E::from(Invalid("gain must be finite and within 0..2")));
        }
        let arrangement = !commands.is_empty()
            && commands.iter().all(|command| {
                matches!(
                    command,
                    Command::ImportPreparedAudioTrackAt { .. }
                        | Command::ImportPreparedAudioClip { .. }
                        | Command::ImportPreparedAudioClipWithOverwrite { .. }
                ) || matches!(
                    command,
                    Command::ImportExistingAudio { placement, .. } if placement.is_arrangement()
                )
            });
        if !arrangement
            && new_track_gain.is_some()
            && commands.iter().any(|command| {
                matches!(command, Command::ImportPreparedAudioTrackAt { .. })
                    || matches!(
                        command,
                        Command::ImportExistingAudio {
                            placement: crate::ExistingAudioPlacement::NewTrack { .. },
                            ..
                        }
                    )
            })
        {
            return Err(E::from(Invalid(
                "new track gain requires an arrangement import batch",
            )));
        }
        if !arrangement && warp != crate::AudioImportWarp::Off {
            return Err(E::from(Invalid(
                "auto-warp requires arrangement audio imports",
            )));
        }
        self.execute_batch_prepared(
            commands,
            arrangement.then_some((warp, new_track_gain)),
            prepare,
        )
    }

    fn execute_batch_prepared<T, E: From<Invalid>>(
        &mut self,
        commands: Vec<Command>,
        arrangement_import: Option<(crate::AudioImportWarp, Option<f32>)>,
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
        if let Some((warp, new_track_gain)) = arrangement_import {
            results = candidate.apply_audio_imports(commands, warp, new_track_gain)?;
        } else {
            let mut alias_changed = false;
            for mut command in commands {
                let alias_plan = crate::clip_alias_edit::Plan::capture(&candidate, &mut command)?;
                results.push(candidate.apply(command)?);
                candidate.prune_deleted_sidechains();
                alias_changed |= alias_plan.finish(&mut candidate)?;
            }
            if alias_changed {
                Project::from_snapshot(candidate.snapshot())?;
            }
        }
        candidate.validate_serial_units()?;
        candidate.validate_source_bindings()?;
        candidate.validate_track_hierarchy()?;
        candidate.validate_changed_midi_occupancy(&self.project)?;
        candidate.validate_changed_arrangement_occupancy(&self.project)?;
        candidate.validate_retained_audio_roots()?;
        candidate.validate_device_asset_references()?;
        candidate.validate_device_automation()?;
        candidate.validate_device_modulation()?;
        candidate.validate_midi_volumes()?;
        candidate.validate_playable_routes()?;
        // Reserve before preparation/commit. No history allocation follows it.
        self.undo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve batch history")))?;
        let prepared = prepare(&candidate, &results)?;
        if candidate != self.project {
            let previous = std::mem::replace(&mut self.project, candidate);
            if self.undo.len() == self.capacity {
                self.undo.pop_front();
            }
            self.undo.push_back(HistoryEntry {
                project: previous,
                midi_effect: None,
                section_publication,
                stopped_bounce: false,
            });
            self.redo.clear();
        }
        Ok((results, prepared))
    }

    pub fn undo(&mut self) -> bool {
        let Some(mut previous) = self.undo.pop_back() else {
            return false;
        };
        previous.project.next_id = previous.project.next_id.max(self.project.next_id);
        previous
            .project
            .retain_gain_touch_epochs_from(&self.project);
        self.redo.push(HistoryEntry {
            project: std::mem::replace(&mut self.project, previous.project),
            midi_effect: previous.midi_effect,
            section_publication: previous.section_publication,
            stopped_bounce: previous.stopped_bounce,
        });
        true
    }

    /// Undo without an external preparation step. Returns false only when
    /// history is empty; reservation failures leave history unchanged.
    /// Callers that must prepare playback before committing use `undo_prepared`.
    pub fn try_undo(&mut self) -> Result<bool, Invalid> {
        self.undo_prepared(|_| Ok::<(), Invalid>(()))
            .map(|result| result.is_some())
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
        candidate.next_id = candidate.next_id.max(self.project.next_id);
        candidate.retain_gain_touch_epochs_from(&self.project);
        self.redo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve redo history")))?;
        let prepared = prepare(&self.project, &candidate)?;
        let Some(_) = self.undo.pop_back() else {
            return Err(E::from(Invalid("undo history changed during preparation")));
        };
        self.redo.push(HistoryEntry {
            project: std::mem::replace(&mut self.project, candidate),
            midi_effect: effect,
            section_publication,
            stopped_bounce,
        });
        Ok(Some(prepared))
    }

    pub fn redo(&mut self) -> bool {
        let Some(mut next) = self.redo.pop() else {
            return false;
        };
        next.project.next_id = next.project.next_id.max(self.project.next_id);
        next.project.retain_gain_touch_epochs_from(&self.project);
        self.undo.push_back(HistoryEntry {
            project: std::mem::replace(&mut self.project, next.project),
            midi_effect: next.midi_effect,
            section_publication: next.section_publication,
            stopped_bounce: next.stopped_bounce,
        });
        true
    }

    /// Redo without an external preparation step. Returns false only when
    /// history is empty; reservation failures leave history unchanged.
    /// Callers that must prepare playback before committing use `redo_prepared`.
    pub fn try_redo(&mut self) -> Result<bool, Invalid> {
        self.redo_prepared(|_| Ok::<(), Invalid>(()))
            .map(|result| result.is_some())
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
        candidate.next_id = candidate.next_id.max(self.project.next_id);
        candidate.retain_gain_touch_epochs_from(&self.project);
        self.undo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve undo history")))?;
        let prepared = prepare(&self.project, &candidate)?;
        let Some(_) = self.redo.pop() else {
            return Err(E::from(Invalid("redo history changed during preparation")));
        };
        if self.undo.len() == self.capacity {
            self.undo.pop_front();
        }
        self.undo.push_back(HistoryEntry {
            project: std::mem::replace(&mut self.project, candidate),
            midi_effect: effect,
            section_publication,
            stopped_bounce,
        });
        Ok(Some(prepared))
    }
}

#[cfg(test)]
mod retained_source_batch_tests {
    use super::*;

    fn editor_with_receiver(retained: bool) -> (Editor, crate::TrackId, crate::SourceId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::AddMidiTrack {
                name: "receiver".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::NativeSource(source) = editor
            .execute(Command::CreateNativeSource {
                name: "source".into(),
                devices: vec![crate::default_instrument_state().unwrap()],
                instrument_index: 0,
            })
            .unwrap()
        else {
            panic!("source")
        };
        editor
            .execute(Command::BindTrackSource {
                track: receiver,
                source: Some(source),
            })
            .unwrap();
        let mut snapshot = editor.project().snapshot();
        if retained {
            snapshot.retained_source_paths.push(receiver.value());
        }
        let editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
        (editor, receiver, source)
    }

    #[test]
    fn batches_validate_retained_receiver_before_preparation_and_preserve_history_on_refusal() {
        for unlink in [false, true] {
            let (mut editor, receiver, source) = editor_with_receiver(true);
            editor
                .execute(Command::RenameTrack {
                    track: receiver,
                    name: "changed".into(),
                })
                .unwrap();
            assert!(editor.undo());
            let before = editor.project().snapshot();
            let undo = editor.can_undo();
            let redo = editor.can_redo();
            let mut called = false;
            let command = if unlink {
                Command::BindTrackSource {
                    track: receiver,
                    source: None,
                }
            } else {
                Command::SetNativeSourceReceiver {
                    source,
                    track: None,
                }
            };
            let result = editor.execute_many_prepared(
                vec![
                    Command::AddTrack {
                        name: "temporary".into(),
                    },
                    command,
                ],
                |_, _| {
                    called = true;
                    Ok::<(), Invalid>(())
                },
            );
            assert!(result.is_err());
            assert!(!called, "invalid candidates must not reach preparation");
            assert_eq!(editor.project().snapshot(), before);
            assert_eq!(editor.can_undo(), undo);
            assert_eq!(editor.can_redo(), redo);
            assert!(editor.redo());
            assert_eq!(editor.project().track(receiver).unwrap().name(), "changed");
        }
    }

    #[test]
    fn valid_retained_and_ordinary_batches_remain_editable_and_undoable() {
        for retained in [false, true] {
            let (mut editor, receiver, source) = editor_with_receiver(retained);
            let before = editor.project().snapshot();
            editor
                .execute_many_prepared(
                    vec![
                        Command::SetNativeSourceReceiver {
                            source,
                            track: None,
                        },
                        Command::SetNativeSourceReceiver {
                            source,
                            track: Some(receiver),
                        },
                        Command::RenameTrack {
                            track: receiver,
                            name: "renamed".into(),
                        },
                    ],
                    |_, _| Ok::<(), Invalid>(()),
                )
                .unwrap();
            assert_eq!(editor.project().track(receiver).unwrap().name(), "renamed");
            assert!(editor.undo());
            assert_eq!(editor.project().snapshot(), before);
            assert!(editor.redo());
            assert_eq!(editor.project().track(receiver).unwrap().name(), "renamed");
        }
    }

    fn add_peer(editor: &mut Editor, source: crate::SourceId) -> crate::TrackId {
        let Created::Track(peer) = editor
            .execute(Command::AddMidiTrack {
                name: "peer".into(),
            })
            .unwrap()
        else {
            panic!("peer")
        };
        editor
            .execute(Command::BindTrackSource {
                track: peer,
                source: Some(source),
            })
            .unwrap();
        peer
    }

    fn group(editor: &mut Editor, tracks: &[crate::TrackId]) -> crate::TrackId {
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: crate::TrackSelectionIds::new(tracks).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        group
    }

    #[test]
    fn deleting_retained_receiver_cannot_strand_surviving_source_peers() {
        for mode in 0..3 {
            let (mut editor, receiver, source) = editor_with_receiver(true);
            let peer = add_peer(&mut editor, source);
            let target = if mode == 2 {
                let inner = group(&mut editor, &[receiver]);
                group(&mut editor, &[inner])
            } else {
                receiver
            };
            editor
                .execute(Command::RenameTrack {
                    track: peer,
                    name: "changed".into(),
                })
                .unwrap();
            assert!(editor.undo());
            let before = editor.project().snapshot();
            let undo = editor.can_undo();
            let mut prepared = false;
            let result = if mode == 1 {
                editor
                    .execute_many_prepared(vec![Command::RemoveTrack(target)], |_, _| {
                        prepared = true;
                        Ok::<(), Invalid>(())
                    })
                    .map(|_| ())
            } else {
                editor
                    .execute_prepared(Command::RemoveTrack(target), |_| {
                        prepared = true;
                        Ok::<(), Invalid>(())
                    })
                    .map(|_| ())
            };
            assert!(result.is_err());
            assert!(!prepared);
            assert_eq!(editor.project().snapshot(), before);
            assert_eq!(editor.can_undo(), undo);
            assert!(editor.redo());
            assert_eq!(editor.project().track(peer).unwrap().name(), "changed");
        }
    }

    #[test]
    fn retained_receiver_and_all_peers_can_be_deleted_together_and_undone() {
        for grouped in [false, true] {
            let (mut editor, receiver, source) = editor_with_receiver(true);
            let peer = add_peer(&mut editor, source);
            let group = grouped.then(|| {
                let inner = group(&mut editor, &[receiver, peer]);
                group(&mut editor, &[inner])
            });
            let before = editor.project().snapshot();
            let command = match group {
                Some(group) => Command::RemoveTrack(group),
                None => Command::RemoveTracks(vec![receiver, peer]),
            };
            editor.execute(command).unwrap();
            assert!(editor.project().track(receiver).is_none());
            assert!(editor.project().track(peer).is_none());
            assert_eq!(
                editor.project().native_source(source).unwrap().receiver(),
                None
            );
            assert!(editor.undo());
            assert_eq!(editor.project().snapshot(), before);
            assert!(editor.redo());
            assert!(editor.project().track(receiver).is_none());
        }
        let (mut editor, receiver, source) = editor_with_receiver(true);
        editor.execute(Command::RemoveTrack(receiver)).unwrap();
        assert_eq!(
            editor.project().native_source(source).unwrap().receiver(),
            None
        );
        let (mut ordinary, receiver, source) = editor_with_receiver(false);
        let peer = add_peer(&mut ordinary, source);
        ordinary.execute(Command::RemoveTrack(receiver)).unwrap();
        assert!(ordinary.project().track(peer).is_some());
        assert_eq!(
            ordinary.project().native_source(source).unwrap().receiver(),
            None
        );
    }
}

#[cfg(test)]
mod prepared_capacity_tests {
    use super::*;

    #[test]
    fn fallible_simple_transitions_report_empty_and_retain_capacity() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
        assert!(!editor.try_undo().unwrap());
        assert!(!editor.try_redo().unwrap());
        for name in ["first", "second"] {
            editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap();
        }
        assert!(editor.try_undo().unwrap());
        assert_eq!(editor.project().tracks().len(), 1);
        assert!(!editor.try_undo().unwrap());
        assert!(editor.try_redo().unwrap());
        assert_eq!(editor.project().tracks().len(), 2);
        assert!(!editor.try_redo().unwrap());
    }

    #[test]
    fn prepared_undo_and_redo_preserve_the_bounded_history() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        for name in ["first", "second", "third"] {
            editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap();
        }
        for expected_tracks in [2, 1] {
            assert!(
                editor
                    .undo_prepared(|_| Ok::<(), Invalid>(()))
                    .unwrap()
                    .is_some()
            );
            assert_eq!(editor.project().tracks().len(), expected_tracks);
        }
        assert!(
            editor
                .undo_prepared(|_| Ok::<(), Invalid>(()))
                .unwrap()
                .is_none()
        );
        for expected_tracks in [2, 3] {
            assert!(
                editor
                    .redo_prepared(|_| Ok::<(), Invalid>(()))
                    .unwrap()
                    .is_some()
            );
            assert_eq!(editor.project().tracks().len(), expected_tracks);
        }
        assert!(
            editor
                .redo_prepared(|_| Ok::<(), Invalid>(()))
                .unwrap()
                .is_none()
        );
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
