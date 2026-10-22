use crate::{Command, Created, Invalid, Project};
use std::collections::VecDeque;

/// Bounded snapshot history on the control thread. Oldest edits expire at capacity.
/// IDs never rewind across undo, redo or branching, so stale references stay stale.
struct HistoryEntry {
    project: Project,
    midi_effect: Option<crate::MidiHistoryEffect>,
}

pub struct Editor {
    project: Project,
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    capacity: usize,
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

    /// Origin of the actual adjacent edge, minted only by canonical single-command execution.
    pub fn next_midi_history_effect(&self, redo: bool) -> Option<crate::MidiHistoryEffect> {
        if redo {
            self.redo.last()
        } else {
            self.undo.back()
        }?
        .midi_effect
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
        candidate.validate_changed_midi_occupancy(&self.project)?;
        candidate.validate_changed_arrangement_occupancy(&self.project)?;
        candidate.validate_device_asset_references()?;
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
        let previous = std::mem::replace(&mut self.project, candidate);
        if self.undo.len() == self.capacity {
            self.undo.pop_front();
        }
        self.undo.push_back(HistoryEntry {
            project: previous,
            midi_effect: effect,
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
        self.execute_batch_prepared(commands, false, prepare)
    }

    /// Import an Arrangement audio batch as one replacement plan. Console imports
    /// retain their ordered cell commands. Only the final candidate is prepared;
    /// intermediate split capacity cannot reject an otherwise admissible batch.
    pub fn execute_audio_imports_prepared<T, E: From<Invalid>>(
        &mut self,
        commands: Vec<Command>,
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
        self.execute_batch_prepared(commands, arrangement, prepare)
    }

    fn execute_batch_prepared<T, E: From<Invalid>>(
        &mut self,
        commands: Vec<Command>,
        arrangement_import: bool,
        prepare: impl FnOnce(&Project, &[Created]) -> Result<T, E>,
    ) -> Result<(Vec<Created>, T), E> {
        if commands.len() > crate::MAX_CLIPS {
            return Err(E::from(Invalid("batch command limit reached")));
        }
        let mut results = Vec::new();
        results
            .try_reserve_exact(commands.len())
            .map_err(|_| E::from(Invalid("could not reserve batch results")))?;
        let mut candidate = self.project.clone();
        if arrangement_import {
            results = candidate.apply_audio_imports(commands)?;
        } else {
            for command in commands {
                results.push(candidate.apply(command)?);
            }
        }
        candidate.validate_changed_midi_occupancy(&self.project)?;
        candidate.validate_changed_arrangement_occupancy(&self.project)?;
        candidate.validate_device_asset_references()?;
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
        });
        Ok(Some(prepared))
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
