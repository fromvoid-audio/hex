use crate::{Command, Created, Invalid, Project};
use std::collections::VecDeque;
use std::sync::Arc;

/// Bounded snapshot history on the control thread. Oldest edits expire at capacity.
/// IDs never rewind across undo, redo or branching, so stale references stay stale.
struct HistoryEntry {
    project: Arc<Project>,
    midi_effect: Option<crate::MidiHistoryEffect>,
    // Structural intent can affect only markers in an empty arrangement.
    // Snapshot audio differences cannot recover the required publication law.
    section_publication: bool,
    // Ephemeral history-edge custody, never saved project state. Both directions
    // replace baked/live source ownership under the stopped Arrangement gate.
    stopped_bounce: bool,
}

/// Validation and diff scope for a single canonical command.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CommandValidationScope {
    /// Pure scalar mixer / mute / solo / send-level edit that cannot alter tracks,
    /// clips, devices, routing topology, or occupancy invariants.
    PureMixer,
    /// Arrangement MIDI note edit that cannot alter audio clips, devices, routing,
    /// or track hierarchy invariants (when clip aliases did not expand).
    MidiNotesOnly,
    /// General command requiring full project validation and structural diff.
    Full,
}

fn command_validation_scope(project: &Project, command: &Command) -> CommandValidationScope {
    match command {
        Command::Gain { .. }
        | Command::GainBy { .. }
        | Command::SetTrackGains { .. }
        | Command::Balance { .. }
        | Command::BalanceBy { .. }
        | Command::MasterGain { .. }
        | Command::MasterGainBy { .. }
        | Command::Mute { .. }
        | Command::ToggleMute(..)
        | Command::ToggleTracksMute(..)
        | Command::ToggleSolo { .. }
        | Command::ToggleSoloTracks { .. }
        | Command::ReturnGain { .. }
        | Command::ReturnMute { .. } => CommandValidationScope::PureMixer,
        // Only a level change on an existing send preserves routing topology.
        Command::SetReturnSend {
            track,
            return_track,
            gain,
        } if *gain > 0.0
            && project.track(*track).is_some_and(|t| {
                t.sends()
                    .iter()
                    .any(|send| send.return_track() == *return_track)
            }) =>
        {
            CommandValidationScope::PureMixer
        }
        Command::AddMidiNote { .. }
        | Command::AddMidiNotes { .. }
        | Command::AddMidiNotesWithExpression { .. }
        | Command::UpdateMidiNote { .. }
        | Command::UpdateMidiNotes { .. }
        | Command::EditMidiNotes { .. }
        | Command::SetMidiNotesActive { .. }
        | Command::RemoveMidiNote(_)
        | Command::RemoveMidiNotes { .. } => CommandValidationScope::MidiNotesOnly,
        _ => CommandValidationScope::Full,
    }
}

/// Fast pre-clone check for common scalar mixer no-ops.
/// Returns `true` only when the target exists and already has the exact requested value,
/// guaranteeing `apply()` would succeed with `Created::None` and zero state change.
fn is_pure_mixer_noop(project: &Project, command: &Command) -> bool {
    match command {
        Command::Gain { track, gain } => project
            .track(*track)
            .is_some_and(|t| crate::track_gain_group::eligible(t) && t.gain() == *gain),
        Command::Balance { track, balance } => {
            balance.is_finite()
                && project
                    .track(*track)
                    .is_some_and(|t| t.balance() == *balance)
        }
        Command::MasterGain { gain } => gain.is_finite() && project.master_gain() == *gain,
        Command::Mute { track, muted } => {
            project.track(*track).is_some_and(|t| t.muted() == *muted)
        }
        Command::ReturnGain { return_track, gain } => {
            gain.is_finite()
                && project
                    .return_tracks()
                    .iter()
                    .find(|bus| bus.id() == *return_track)
                    .is_some_and(|bus| bus.gain() == *gain)
        }
        Command::ReturnMute {
            return_track,
            muted,
        } => project
            .return_tracks()
            .iter()
            .find(|bus| bus.id() == *return_track)
            .is_some_and(|bus| bus.muted() == *muted),
        _ => false,
    }
}

/// O(tracks + returns) diff for `PureMixer` commands, avoiding full-project `PartialEq`.
fn pure_mixer_changed(before: &Project, after: &Project) -> bool {
    if before.master_gain() != after.master_gain()
        || before.tracks().len() != after.tracks().len()
        || before.return_tracks().len() != after.return_tracks().len()
    {
        return true;
    }
    if before.tracks().iter().zip(after.tracks()).any(|(a, b)| {
        a.gain() != b.gain()
            || a.balance() != b.balance()
            || a.muted() != b.muted()
            || a.soloed() != b.soloed()
            || a.sends() != b.sends()
    }) {
        return true;
    }
    before
        .return_tracks()
        .iter()
        .zip(after.return_tracks())
        .any(|(a, b)| a.gain() != b.gain() || a.muted() != b.muted())
}

/// Scoped diff for `MidiNotesOnly` commands without alias expansion.
fn midi_notes_changed(before: &Project, after: &Project) -> bool {
    before.next_id != after.next_id
        || before.tracks().len() != after.tracks().len()
        || before.session() != after.session()
        || before
            .tracks()
            .iter()
            .zip(after.tracks())
            .any(|(a, b)| a.midi_clips() != b.midi_clips())
}

fn needs_history_normalization(target: &Project, current: &Project) -> bool {
    target.next_id < current.next_id
        || target.tracks.iter().any(|track| {
            current
                .track(track.id)
                .is_some_and(|old| track.gain_lane.recording_epoch < old.gain_lane.recording_epoch)
        })
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

/// One validated edit and its reserved history step. It can outlive the
/// Editor borrow while an external owner prepares or applies the candidate.
/// Dropping it leaves the authored project and history unchanged.
#[must_use]
pub struct StagedProjectEdit {
    editor_identity: Arc<()>,
    history_revision: u64,
    project: Arc<Project>,
    created: Created,
    changed: bool,
    midi_effect: Option<crate::MidiHistoryEffect>,
    section_publication: bool,
    stopped_bounce: bool,
}

impl StagedProjectEdit {
    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn project_arc(&self) -> Arc<Project> {
        Arc::clone(&self.project)
    }

    pub fn changed(&self) -> bool {
        self.changed
    }
}

/// One exact Undo or Redo target and its reserved history transfer. Dropping
/// it leaves the canonical project and both history stacks unchanged.
#[must_use]
pub struct StagedProjectHistory {
    editor_identity: Arc<()>,
    history_revision: u64,
    project: Arc<Project>,
    redo: bool,
}

impl StagedProjectHistory {
    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn project_arc(&self) -> Arc<Project> {
        Arc::clone(&self.project)
    }
}

pub struct Editor {
    project: Arc<Project>,
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    capacity: usize,
    identity: Arc<()>,
    history_revision: u64,
}
impl Editor {
    /// Start history for an already validated project, including retained source
    /// paths. Subsequent edits validate source and receiver bindings before commit.
    pub fn new(project: Project, capacity: usize) -> Result<Self, Invalid> {
        if !(1..=128).contains(&capacity) {
            return Err(Invalid("history capacity must be 1..128"));
        }
        Ok(Self {
            project: Arc::new(project),
            undo: VecDeque::new(),
            redo: Vec::new(),
            capacity,
            identity: Arc::new(()),
            history_revision: 0,
        })
    }
    pub fn project(&self) -> &Project {
        &self.project
    }
    /// Immutable shared handle to the current canonical project without deep cloning.
    pub fn project_arc(&self) -> Arc<Project> {
        Arc::clone(&self.project)
    }
    /// Monotonic history revision incremented on every committed edit, undo, or redo.
    pub fn history_revision(&self) -> u64 {
        self.history_revision
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
        let original =
            outgoing.map(|project| std::mem::replace(&mut self.project, Arc::new(project)));
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
        if candidate == *self.project {
            return Ok(None);
        }
        self.undo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve sample rate history")))?;
        let candidate = Arc::new(candidate);
        let prepared = prepare(&candidate)?;
        self.commit_candidate(candidate, None, false, false);
        Ok(Some(prepared))
    }

    fn commit_candidate(
        &mut self,
        candidate: Arc<Project>,
        midi_effect: Option<crate::MidiHistoryEffect>,
        section_publication: bool,
        stopped_bounce: bool,
    ) {
        let previous = std::mem::replace(&mut self.project, candidate);
        self.push_undo(previous, midi_effect, section_publication, stopped_bounce);
    }

    fn push_undo(
        &mut self,
        previous: Arc<Project>,
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
        self.advance_history_revision();
    }

    fn advance_history_revision(&mut self) {
        self.history_revision = self.history_revision.saturating_add(1);
    }

    fn renew_history_identity_if_exhausted(&mut self) {
        // Renew before admission so the commit does not allocate a new key and
        // an exhausted revision cannot alias older staged operations.
        if self.history_revision == u64::MAX {
            self.identity = Arc::new(());
            self.history_revision = 0;
        }
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
        self.project = Arc::new(candidate);
        self.push_undo(Arc::new(previous), None, false, false);
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
        let edit = self.stage_transition(command, require_midi)?;
        let prepared = prepare(&self.project, edit.project())?;
        Ok((self.commit_staged_edit(edit)?, prepared))
    }

    /// Validate one command and reserve history before a delayed consumer
    /// can publish its graph. The canonical project remains available to save.
    pub fn stage_edit(&mut self, command: Command) -> Result<StagedProjectEdit, Invalid> {
        self.stage_transition(command, false)
    }

    /// Commit exactly the prepared edit, consuming its reserved history step.
    /// The caller owns the external approval and must prevent intervening edits
    /// once publication may install. A stale or foreign candidate is refused.
    pub fn commit_staged_edit(&mut self, edit: StagedProjectEdit) -> Result<Created, Invalid> {
        if edit.history_revision != self.history_revision
            || !Arc::ptr_eq(&edit.editor_identity, &self.identity)
        {
            return Err(Invalid("staged edit no longer matches project history"));
        }
        // No-op controls should neither add history nor erase a redo branch.
        if edit.changed {
            self.commit_candidate(
                edit.project,
                edit.midi_effect,
                edit.section_publication,
                edit.stopped_bounce,
            );
        }
        Ok(edit.created)
    }

    /// Validate and apply one preview command without touching history or cloning
    /// `Editor`. Reuses the current `Arc<Project>` on no-ops and performs at most
    /// one `Project` clone otherwise.
    pub fn stage_preview(&self, command: Command) -> Result<Arc<Project>, Invalid> {
        let scope = command_validation_scope(&self.project, &command);
        if scope == CommandValidationScope::PureMixer && is_pure_mixer_noop(&self.project, &command)
        {
            return Ok(Arc::clone(&self.project));
        }
        let mut command = command;
        let alias_plan = crate::clip_alias_edit::Plan::capture(&self.project, &mut command)?;
        let mut candidate = (*self.project).clone();
        let _ = candidate.apply(command)?;
        let alias_changed = alias_plan.finish(&mut candidate)?;
        let changed = match scope {
            CommandValidationScope::PureMixer => pure_mixer_changed(&self.project, &candidate),
            CommandValidationScope::MidiNotesOnly if !alias_changed => {
                candidate.validate_changed_midi_occupancy(&self.project)?;
                candidate.validate_midi_volumes()?;
                midi_notes_changed(&self.project, &candidate)
            }
            _ => {
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
                candidate != *self.project
            }
        };
        if changed {
            Ok(Arc::new(candidate))
        } else {
            Ok(Arc::clone(&self.project))
        }
    }

    fn stage_transition(
        &mut self,
        command: Command,
        require_midi: bool,
    ) -> Result<StagedProjectEdit, Invalid> {
        let scope = command_validation_scope(&self.project, &command);
        if !require_midi
            && scope == CommandValidationScope::PureMixer
            && is_pure_mixer_noop(&self.project, &command)
        {
            self.renew_history_identity_if_exhausted();
            return Ok(StagedProjectEdit {
                editor_identity: Arc::clone(&self.identity),
                history_revision: self.history_revision,
                project: Arc::clone(&self.project),
                created: Created::None,
                changed: false,
                midi_effect: None,
                section_publication: false,
                stopped_bounce: false,
            });
        }
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
            return Err(Invalid("unsupported midi edit command"));
        }
        let mut candidate = (*self.project).clone();
        let result = candidate.apply(command)?;
        let alias_changed = alias_plan.finish(&mut candidate)?;
        let changed = match scope {
            CommandValidationScope::PureMixer => pure_mixer_changed(&self.project, &candidate),
            CommandValidationScope::MidiNotesOnly if !alias_changed => {
                candidate.validate_changed_midi_occupancy(&self.project)?;
                candidate.validate_midi_volumes()?;
                midi_notes_changed(&self.project, &candidate)
            }
            _ => {
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
                candidate != *self.project
            }
        };
        let effect = plan.and_then(|p| p.finish(result, &self.project, &candidate));
        if require_midi && changed && effect.is_none() {
            return Err(Invalid("midi command changed unsupported project state"));
        }
        if changed {
            self.undo
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve edit history"))?;
        }
        self.renew_history_identity_if_exhausted();
        let project = if changed {
            Arc::new(candidate)
        } else {
            Arc::clone(&self.project)
        };
        Ok(StagedProjectEdit {
            editor_identity: Arc::clone(&self.identity),
            history_revision: self.history_revision,
            project,
            created: result,
            changed,
            midi_effect: effect,
            section_publication,
            stopped_bounce,
        })
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
        let mut candidate = (*self.project).clone();
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
        let changed = candidate != *self.project;
        let candidate = if changed {
            Arc::new(candidate)
        } else {
            Arc::clone(&self.project)
        };
        let prepared = prepare(&candidate, &results)?;
        if changed {
            self.commit_candidate(candidate, None, section_publication, false);
        }
        Ok((results, prepared))
    }

    pub fn undo(&mut self) -> bool {
        let Some(mut previous) = self.undo.pop_back() else {
            return false;
        };
        if needs_history_normalization(&previous.project, &self.project) {
            let project = Arc::make_mut(&mut previous.project);
            project.next_id = project.next_id.max(self.project.next_id);
            project.retain_gain_touch_epochs_from(&self.project);
        }
        self.redo.push(HistoryEntry {
            project: std::mem::replace(&mut self.project, previous.project),
            midi_effect: previous.midi_effect,
            section_publication: previous.section_publication,
            stopped_bounce: previous.stopped_bounce,
        });
        self.advance_history_revision();
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
        self.prepare_history_transition(false, prepare)
    }

    pub fn redo(&mut self) -> bool {
        let Some(mut next) = self.redo.pop() else {
            return false;
        };
        if needs_history_normalization(&next.project, &self.project) {
            let project = Arc::make_mut(&mut next.project);
            project.next_id = project.next_id.max(self.project.next_id);
            project.retain_gain_touch_epochs_from(&self.project);
        }
        self.undo.push_back(HistoryEntry {
            project: std::mem::replace(&mut self.project, next.project),
            midi_effect: next.midi_effect,
            section_publication: next.section_publication,
            stopped_bounce: next.stopped_bounce,
        });
        self.advance_history_revision();
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
        self.prepare_history_transition(true, prepare)
    }

    fn prepare_history_transition<T, E: From<Invalid>>(
        &mut self,
        redo: bool,
        prepare: impl FnOnce(&Project, &Project) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        let Some(history) = self.stage_history(redo)? else {
            return Ok(None);
        };
        let prepared = prepare(&self.project, history.project())?;
        self.commit_staged_history(history)?;
        Ok(Some(prepared))
    }

    /// Normalize the exact adjacent history target and reserve its inverse
    /// before an external owner prepares or installs that target.
    pub fn stage_history(&mut self, redo: bool) -> Result<Option<StagedProjectHistory>, Invalid> {
        let target = if redo {
            self.redo.last()
        } else {
            self.undo.back()
        };
        let Some(target) = target else {
            return Ok(None);
        };
        let mut project = Arc::clone(&target.project);
        if needs_history_normalization(&project, &self.project) {
            let normalized = Arc::make_mut(&mut project);
            normalized.next_id = normalized.next_id.max(self.project.next_id);
            normalized.retain_gain_touch_epochs_from(&self.project);
        }
        if redo {
            self.undo
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve undo history"))?;
        } else {
            self.redo
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve redo history"))?;
        }
        self.renew_history_identity_if_exhausted();
        Ok(Some(StagedProjectHistory {
            editor_identity: Arc::clone(&self.identity),
            history_revision: self.history_revision,
            project,
            redo,
        }))
    }

    /// Stage a normalized history edge that changes only one Track's device
    /// order. Use the stored target because a multi-device reorder may have
    /// no inverse expressible by one selection and insertion point.
    pub fn stage_track_device_reorder_history(
        &mut self,
        redo: bool,
    ) -> Result<Option<(crate::TrackId, StagedProjectHistory)>, Invalid> {
        let Some(track) = self
            .inspect_history(redo, |current, target| {
                if current.tracks().len() != target.tracks().len() {
                    return None;
                }
                let mut changed = None;
                for (old, new) in current.tracks().iter().zip(target.tracks()) {
                    if old.id() != new.id() {
                        return None;
                    }
                    let (old_devices, new_devices) = (old.devices(), new.devices());
                    if old_devices == new_devices {
                        continue;
                    }
                    if changed.replace(old.id()).is_some()
                        || old_devices.len() != new_devices.len()
                        || old_devices
                            .iter()
                            .any(|device| !new_devices.contains(device))
                    {
                        return None;
                    }
                }
                changed
            })
            .flatten()
        else {
            return Ok(None);
        };
        let Some(mut edge) = self.stage_history(redo)? else {
            return Ok(None);
        };
        let current = self
            .project
            .track(track)
            .ok_or(Invalid("reordered track changed"))?;
        // Temporarily mask only the order on the private target, then put
        // back the exact history payload before giving it to the caller.
        let project = Arc::make_mut(&mut edge.project);
        let reordered = std::mem::replace(
            &mut project.track_mut(track)?.devices,
            current.devices().to_vec(),
        );
        let exact = *project == *self.project;
        project.track_mut(track)?.devices = reordered;
        Ok(exact.then_some((track, edge)))
    }

    /// Commit the same staged history target after external approval. Prevent
    /// intervening edits once its external publication may install.
    pub fn commit_staged_history(&mut self, history: StagedProjectHistory) -> Result<(), Invalid> {
        if history.history_revision != self.history_revision
            || !Arc::ptr_eq(&history.editor_identity, &self.identity)
        {
            return Err(Invalid("staged history no longer matches editor history"));
        }
        let edge = if history.redo {
            self.redo.pop()
        } else {
            self.undo.pop_back()
        }
        .ok_or(Invalid("staged history target is unavailable"))?;
        let inverse = HistoryEntry {
            project: std::mem::replace(&mut self.project, history.project),
            ..edge
        };
        if history.redo {
            if self.undo.len() == self.capacity {
                self.undo.pop_front();
            }
            self.undo.push_back(inverse);
        } else {
            self.redo.push(inverse);
        }
        self.advance_history_revision();
        Ok(())
    }
}

#[cfg(test)]
mod staged_edit_tests {
    use super::*;

    fn insert() -> Command {
        Command::InsertMainDevice {
            before: None,
            state: crate::default_instrument_state().unwrap(),
        }
    }

    #[test]
    fn staged_insert_is_private_until_its_one_history_commit() {
        let original = Project::new(48_000).unwrap();
        let mut editor = Editor::new(original.clone(), 2).unwrap();
        let abandoned = editor.stage_edit(insert()).unwrap();
        let candidate = abandoned.project().clone();
        assert_eq!(candidate.main_devices().len(), 1);
        assert_eq!(editor.project(), &original);
        assert!(!editor.can_undo());
        assert!(!editor.can_redo());
        drop(abandoned);
        assert_eq!(editor.project(), &original);

        let insertion = editor.stage_edit(insert()).unwrap();
        assert_eq!(insertion.project(), &candidate);
        assert_eq!(
            editor.commit_staged_edit(insertion).unwrap(),
            Created::Device(candidate.main_devices()[0].id())
        );
        assert_eq!(editor.project(), &candidate);
        assert!(editor.undo());
        assert!(editor.project().main_devices().is_empty());
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project(), &candidate);
    }

    #[test]
    fn staged_history_keeps_undo_and_redo_private_until_exact_commit() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        assert!(editor.stage_history(false).unwrap().is_none());
        editor.execute(insert()).unwrap();
        let before = editor.project().clone();

        let abandoned = editor.stage_history(false).unwrap().unwrap();
        assert!(abandoned.project().main_devices().is_empty());
        assert_eq!(abandoned.project().next_identity(), before.next_identity());
        drop(abandoned);
        assert_eq!(editor.project(), &before);
        assert!(editor.can_undo() && !editor.can_redo());

        let undo = editor.stage_history(false).unwrap().unwrap();
        let expected = undo.project().clone();
        editor.commit_staged_history(undo).unwrap();
        assert_eq!(editor.project(), &expected);
        assert!(!editor.can_undo() && editor.can_redo());

        let redo = editor.stage_history(true).unwrap().unwrap();
        assert_eq!(redo.project(), &before);
        assert_eq!(editor.project(), &expected);
        editor.commit_staged_history(redo).unwrap();
        assert_eq!(editor.project(), &before);
        assert!(editor.can_undo() && !editor.can_redo());
    }

    #[test]
    fn staged_reorder_history_restores_interleaved_order_and_excludes_compound_edits() {
        use crate::{DeviceChainOwner, DeviceSelectionIds};

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let mut devices = Vec::new();
        for _ in 0..4 {
            let Created::Device(device) = editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: crate::default_instrument_state().unwrap(),
                })
                .unwrap()
            else {
                panic!("device");
            };
            devices.push(device);
        }
        let reorder = || Command::ReorderDevices {
            owner: DeviceChainOwner::Track(track),
            selected: DeviceSelectionIds::new(&[devices[0], devices[2]]).unwrap(),
            before: None,
        };
        let original = editor.project().clone();
        editor.execute(reorder()).unwrap();
        let reordered = editor.project().clone();
        let reordered_ids: Vec<_> = reordered
            .track(track)
            .unwrap()
            .devices()
            .iter()
            .map(crate::Device::id)
            .collect();
        assert_eq!(
            reordered_ids,
            [devices[1], devices[3], devices[0], devices[2]]
        );

        let (owner, undo) = editor
            .stage_track_device_reorder_history(false)
            .unwrap()
            .unwrap();
        assert_eq!(owner, track);
        assert_eq!(undo.project(), &original);
        assert_eq!(editor.project(), &reordered);
        editor.commit_staged_history(undo).unwrap();
        let (owner, redo) = editor
            .stage_track_device_reorder_history(true)
            .unwrap()
            .unwrap();
        assert_eq!(owner, track);
        assert_eq!(redo.project(), &reordered);
        assert_eq!(editor.project(), &original);
        drop(redo);
        assert!(editor.can_redo());
        editor
            .execute_many_prepared::<(), Invalid>(
                vec![
                    reorder(),
                    Command::SetTrackGains {
                        gains: vec![(track, 0.5)],
                    },
                ],
                |_, _| Ok(()),
            )
            .unwrap();
        let compound = editor.project().clone();
        assert!(
            editor
                .stage_track_device_reorder_history(false)
                .unwrap()
                .is_none()
        );
        assert_eq!(editor.project(), &compound);
        assert!(editor.can_undo());
        assert!(!editor.can_redo());
    }

    #[test]
    fn staged_history_refuses_foreign_and_obsolete_targets() {
        let initial = Project::new(48_000).unwrap();
        let mut editor = Editor::new(initial.clone(), 2).unwrap();
        let mut other = Editor::new(initial, 2).unwrap();
        editor.execute(insert()).unwrap();
        other.execute(insert()).unwrap();
        let before = editor.project().clone();

        let foreign = editor.stage_history(false).unwrap().unwrap();
        assert!(other.commit_staged_history(foreign).is_err());
        assert_eq!(other.project(), &before);
        assert!(other.can_undo() && !other.can_redo());

        let stale = editor.stage_history(false).unwrap().unwrap();
        assert!(editor.try_undo().unwrap());
        assert!(editor.try_redo().unwrap());
        assert!(editor.commit_staged_history(stale).is_err());
        assert_eq!(editor.project(), &before);
        assert!(editor.can_undo() && !editor.can_redo());
    }

    #[test]
    fn staged_edit_cannot_cross_editor_or_return_to_an_old_history_edge() {
        let original = Project::new(48_000).unwrap();
        let mut editor = Editor::new(original.clone(), 2).unwrap();
        let mut other = Editor::new(original.clone(), 2).unwrap();
        let foreign = editor.stage_edit(insert()).unwrap();
        assert!(other.commit_staged_edit(foreign).is_err());
        assert_eq!(other.project(), &original);
        assert!(!other.can_undo());

        let stale = editor.stage_edit(insert()).unwrap();
        editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
        assert!(editor.try_undo().unwrap());
        assert_eq!(editor.project(), &original);
        assert!(editor.commit_staged_edit(stale).is_err());
        assert!(editor.can_redo());

        let before_redo = editor.stage_edit(insert()).unwrap();
        assert!(editor.try_redo().unwrap());
        assert!(editor.commit_staged_edit(before_redo).is_err());
        assert!(editor.project().main_devices().is_empty());
    }

    #[test]
    fn a_staged_no_op_keeps_redo_and_a_batch_invalidates_older_candidates() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
        assert!(editor.undo());
        let no_op = editor
            .stage_edit(Command::MasterGain { gain: 1.0 })
            .unwrap();
        editor.commit_staged_edit(no_op).unwrap();
        assert!(!editor.can_undo());
        assert!(editor.can_redo());

        let older = editor.stage_edit(insert()).unwrap();
        editor
            .execute_many_prepared(vec![Command::MasterGain { gain: 0.7 }], |_, _| {
                Ok::<_, Invalid>(())
            })
            .unwrap();
        assert!(editor.commit_staged_edit(older).is_err());
        assert!(editor.project().main_devices().is_empty());
        assert!(!editor.can_redo());
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
            Arc::make_mut(&mut editor.project).tracks[0]
                .gain_lane
                .recording_epoch = 9;
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
            Arc::make_mut(&mut editor.project).tracks[0]
                .gain_lane
                .recording_epoch = 10;
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

    #[test]
    fn arc_sharing_and_fast_validation_preserve_noop_preview_and_history_invariants() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "synth".into(),
            })
            .unwrap()
        else {
            panic!("track expected");
        };
        let baseline = editor.project_arc();
        let rev_before = editor.history_revision();

        // Pure mixer no-op reuses exact Arc without cloning or advancing history.
        let noop = editor
            .stage_edit(Command::Mute {
                track,
                muted: false,
            })
            .unwrap();
        assert!(!noop.changed());
        assert!(Arc::ptr_eq(&noop.project_arc(), &baseline));
        editor.commit_staged_edit(noop).unwrap();
        assert_eq!(editor.history_revision(), rev_before);
        assert!(Arc::ptr_eq(&editor.project_arc(), &baseline));

        // Preview no-op reuses exact Arc; real preview returns distinct Arc without touching history.
        let preview_noop = editor
            .stage_preview(Command::Gain { track, gain: 1.0 })
            .unwrap();
        assert!(Arc::ptr_eq(&preview_noop, &baseline));
        let preview_changed = editor
            .stage_preview(Command::Gain { track, gain: 0.5 })
            .unwrap();
        assert!(!Arc::ptr_eq(&preview_changed, &baseline));
        assert_eq!(preview_changed.track(track).unwrap().gain(), 0.5);
        assert_eq!(editor.history_revision(), rev_before);
        assert!(Arc::ptr_eq(&editor.project_arc(), &baseline));

        // Invalid pure-mixer value still fails cleanly.
        assert!(
            editor
                .stage_preview(Command::Gain {
                    track,
                    gain: f32::NAN,
                })
                .is_err()
        );

        // Real mixer edit commits, and Undo returns the exact baseline Arc without re-cloning.
        editor
            .execute(Command::Mute { track, muted: true })
            .unwrap();
        assert!(editor.project().track(track).unwrap().muted());
        assert!(!Arc::ptr_eq(&editor.project_arc(), &baseline));
        assert!(editor.undo());
        assert!(Arc::ptr_eq(&editor.project_arc(), &baseline));
        assert!(editor.redo());
        assert!(editor.project().track(track).unwrap().muted());

        // Single-clip MIDI note add/update/undo fast-path preserves exact behavior.
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: crate::BeatTime::ZERO,
                length: crate::BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("midi clip expected");
        };
        let Created::Note(note) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: crate::BeatTime::ZERO,
                duration: crate::BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("note expected");
        };
        let after_add = editor.project_arc();
        editor
            .execute(Command::UpdateMidiNote {
                note,
                start: crate::BeatTime::ONE,
                duration: crate::BeatTime::ONE,
                pitch: 64,
                channel: 0,
                velocity: 96,
            })
            .unwrap();
        assert_eq!(
            editor.project().track(track).unwrap().midi_clips()[0].notes()[0].pitch(),
            64
        );
        assert!(editor.undo());
        assert!(Arc::ptr_eq(&editor.project_arc(), &after_add));

        // Session/Console MIDI clip note updates must also be detected as changed (never treated as no-ops).
        let Created::Scene(scene) = editor
            .execute(Command::AddScene { name: "s".into() })
            .unwrap()
        else {
            panic!("scene expected");
        };
        let Created::SessionClip(session_clip) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "session".into(),
                source_length: crate::BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("session clip expected");
        };
        let Created::Note(session_note) = editor
            .execute(Command::AddMidiNote {
                clip: session_clip,
                start: crate::BeatTime::ZERO,
                duration: crate::BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("session note expected");
        };
        let before_session_update = editor.project_arc();
        let staged = editor
            .stage_edit(Command::UpdateMidiNote {
                note: session_note,
                start: crate::BeatTime::ONE,
                duration: crate::BeatTime::ONE,
                pitch: 67,
                channel: 0,
                velocity: 110,
            })
            .unwrap();
        assert!(staged.changed());
        assert!(!Arc::ptr_eq(&staged.project_arc(), &before_session_update));
        editor.commit_staged_edit(staged).unwrap();
        assert_eq!(
            editor.project().midi_source(session_clip).unwrap().notes()[0].pitch(),
            67
        );
    }
}
