//! Correlate a single selected group with the existing ordered reader and
//! copied Main witness. SDK cohorts order callbacks; each member's Begin
//! survives move cohorts. Only Main supplies the take's value and time.
use hex_audio::foreign_parameter_delivery::editor::{ConfirmedQuantum, Target};
use hex_audio::plugin_worker::{ParameterEdit, ParameterEditKind as Kind};
use hex_session::service::automation_draft::{
    DraftClock, GroupApplication, GroupDraftEvent, Vst3GroupTarget,
};
use std::collections::VecDeque;

use hex_session::service::MAX_VST3_EDITOR_GROUP_MEMBERS as MEMBERS;
// Main's 256 compensated quanta and its bounded receipt queue may both
// contain observations while the sole controller reader is already ahead.
const ORDERED_POINTS: usize =
    (hex_audio::foreign_parameter_delivery::editor::MAX_EDITOR_COMPENSATED_INPUT_QUANTA
        + hex_audio::foreign_parameter_delivery::editor::MAX_EDITOR_MAIN_RECEIPTS
        + 4)
        * MEMBERS;

#[derive(Clone, Copy, Default)]
struct Member {
    begin: Option<u64>,
    end: Option<(u64, u64)>,
    latest: Option<GroupDraftEvent>,
}

#[derive(Clone, Copy)]
struct Expected {
    parameter: u32,
    gesture: u64,
    sequence: u64,
    frame: u64,
    value: f64,
}

#[derive(Clone, Copy)]
struct Fact {
    parameter: u32,
    gesture: u64,
    sequence: u64,
    application: GroupApplication,
}

#[allow(clippy::large_enum_variant)] // Keep one bounded copied Main batch.
pub(super) enum Quantum {
    Unrelated,
    WaitingForOrder,
    Applied([Option<GroupDraftEvent>; MEMBERS]),
}

pub(super) struct GroupCorrelation {
    target: Target,
    group: Vst3GroupTarget,
    members: [Member; MEMBERS],
    open_cohort: Option<u64>,
    open_has_member: bool,
    open_has_other: bool,
    first_cohort: Option<u64>,
    ordered_through: u64,
    pending: VecDeque<Expected>,
    latest: Option<GroupDraftEvent>,
    poisoned: bool,
}

impl GroupCorrelation {
    pub(super) fn new(target: Target, group: Vst3GroupTarget) -> Self {
        Self {
            target,
            group,
            members: [Member::default(); MEMBERS],
            open_cohort: None,
            open_has_member: false,
            open_has_other: false,
            first_cohort: None,
            ordered_through: 0,
            pending: VecDeque::with_capacity(ORDERED_POINTS),
            latest: None,
            poisoned: false,
        }
    }

    pub(super) fn target(&self) -> Target {
        self.target
    }

    /// False requests cancellation, but previously witnessed Main rows still
    /// flow through the owner before the interruption can be published.
    pub(super) fn ordered(&mut self, edit: ParameterEdit) -> bool {
        if self.ordered_inner(edit).is_err() {
            self.poisoned = true;
        }
        !self.poisoned
    }

    fn ordered_inner(&mut self, edit: ParameterEdit) -> Result<(), ()> {
        if edit.sequence <= self.ordered_through {
            return Err(());
        }
        self.ordered_through = edit.sequence;
        match edit.kind {
            Kind::GroupBegin => {
                self.open_has_member = false;
                self.open_has_other = false;
                if edit.sequence != edit.gesture || self.open_cohort.replace(edit.gesture).is_some()
                {
                    return Err(());
                }
            }
            Kind::GroupEnd | Kind::GroupAborted => {
                if self.open_cohort.take() != Some(edit.gesture) || edit.kind == Kind::GroupAborted
                {
                    return Err(());
                }
                if self.first_cohort.is_some() {
                    // A completed cohort with some actual releases cannot
                    // wait for a different group to supply missing Begins.
                    let active = &self.members[..self.group.ids().len()];
                    if active.iter().any(|member| member.end.is_some())
                        && active.iter().any(|member| member.begin.is_none())
                    {
                        return Err(());
                    }
                }
            }
            Kind::Begin | Kind::Perform | Kind::End => {
                let slot = self.group.ids().binary_search(&edit.id);
                if self.open_cohort.is_some() {
                    self.open_has_member |= slot.is_ok();
                    self.open_has_other |= slot.is_err();
                    // A cohort with another parameter is not this entire
                    // selected group. Keep actual rows but refuse completion.
                    self.poisoned |= self.open_has_member && self.open_has_other;
                }
                let Ok(slot) = slot else {
                    return Ok(());
                };
                let member = &mut self.members[slot];
                match edit.kind {
                    Kind::Begin => {
                        if let Some(cohort) = self.open_cohort
                            && member.begin.is_none()
                        {
                            let first = *self.first_cohort.get_or_insert(cohort);
                            member.begin = Some(edit.gesture);
                            if first != cohort {
                                return Err(());
                            }
                        }
                    }
                    Kind::Perform if member.begin == Some(edit.gesture) => {
                        if self.open_cohort.is_none()
                            || member.end.is_some()
                            || self.pending.len() == ORDERED_POINTS
                        {
                            return Err(());
                        }
                        self.pending.push_back(Expected {
                            parameter: edit.id,
                            gesture: edit.gesture,
                            sequence: edit.sequence,
                            frame: edit.first_frame,
                            value: edit.normalized,
                        });
                    }
                    Kind::End if member.begin == Some(edit.gesture) => {
                        if member
                            .end
                            .replace((edit.sequence, edit.first_frame))
                            .is_some()
                        {
                            return Err(());
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub(super) fn confirmed(
        &mut self,
        main: ConfirmedQuantum,
        closed: bool,
    ) -> Result<Quantum, &'static str> {
        if main.target != self.target
            || main.input.binding != self.target.binding
            || main.input.scope != self.target.scope
            || main.input.lost_origin
        {
            self.poisoned = true;
            return Err("grouped vst3 Main witness has a stale source");
        }
        let values = main.input.performs.as_slice();
        let ends = main.input.restorations.as_slice();
        if values.is_empty() && ends.is_empty() {
            return Ok(Quantum::Unrelated);
        }
        if values.len() + ends.len() > MEMBERS {
            self.poisoned = true;
            return Err("grouped vst3 Main witness exceeds its member bound");
        }
        let Some(origin) = main
            .input
            .origin
            .filter(|origin| origin.performance_frame <= main.main_performance_frame)
        else {
            self.poisoned = true;
            return Err("grouped vst3 main witness has no compensated origin");
        };
        let clock = DraftClock {
            performance_frame: origin.performance_frame,
            content_frame: origin.arrangement_frame,
            loop_pass: origin.loop_pass,
        };
        let result = self.confirmed_facts(
            main.input.first_frame,
            clock,
            closed,
            values
                .iter()
                .map(|point| Fact {
                    parameter: point.id,
                    gesture: point.gesture,
                    sequence: point.sequence,
                    application: GroupApplication::Value(point.normalized),
                })
                .chain(ends.iter().map(|end| Fact {
                    parameter: end.parameter,
                    gesture: end.gesture,
                    sequence: end.sequence,
                    application: GroupApplication::Restored,
                })),
        );
        self.poisoned |= result.is_err();
        result
    }

    fn confirmed_facts(
        &mut self,
        first_frame: u64,
        clock: DraftClock,
        closed: bool,
        facts: impl Iterator<Item = Fact> + Clone,
    ) -> Result<Quantum, &'static str> {
        if !closed
            && facts
                .clone()
                .any(|fact| fact.sequence > self.ordered_through)
        {
            return Ok(Quantum::WaitingForOrder);
        }
        let mut members = self.members;
        let mut applied = [None; MEMBERS];
        let mut seen = [false; MEMBERS];
        let mut latest = self.latest;
        let mut order_incomplete = false;
        let at_or_before = self
            .pending
            .iter()
            .take_while(|fact| fact.frame <= first_frame)
            .count();
        for (fact, output) in facts.zip(applied.iter_mut()) {
            let slot = self
                .group
                .ids()
                .binary_search(&fact.parameter)
                .map_err(|_| "grouped vst3 Main witness changed its member set")?;
            let member = &mut members[slot];
            let unread = fact.sequence > self.ordered_through && closed;
            if seen[slot]
                || fact.gesture == 0
                || fact.sequence <= fact.gesture
                || member.begin.is_some_and(|begin| begin != fact.gesture)
                || member.latest.is_some_and(|old| {
                    old.sdk_sequence >= fact.sequence
                        || old.application == GroupApplication::Restored
                        || old.clock.performance_frame >= clock.performance_frame
                })
            {
                return Err("grouped vst3 Main witness repeats or changes a member");
            }
            let exact = match fact.application {
                GroupApplication::Value(value) => {
                    self.pending.iter().take(at_or_before).any(|expected| {
                        expected.frame == first_frame
                            && expected.parameter == fact.parameter
                            && expected.gesture == fact.gesture
                            && expected.sequence == fact.sequence
                            && expected.value.to_bits() == value.to_bits()
                    })
                }
                GroupApplication::Restored => {
                    member.latest.is_some()
                        && member.end.is_some_and(|(seq, frame)| {
                            seq == fact.sequence && frame <= first_frame
                        })
                }
            };
            if !exact
                && !(unread
                    && (fact.application != GroupApplication::Restored || member.latest.is_some()))
            {
                return Err("grouped vst3 Main witness has no matching original SDK observation");
            }
            order_incomplete |= !exact;
            let ordinal = latest
                .map_or(Some(1), |old| old.ordinal.checked_add(1))
                .ok_or("grouped vst3 draft ordinal exhausted")?;
            let row = GroupDraftEvent {
                ordinal,
                parameter: fact.parameter,
                sdk_sequence: fact.sequence,
                sdk_gesture: fact.gesture,
                clock,
                application: fact.application,
            };
            member.begin = Some(fact.gesture);
            member.latest = Some(row);
            latest = Some(row);
            *output = Some(row);
            seen[slot] = true;
        }
        self.members = members;
        self.latest = latest;
        self.poisoned |= order_incomplete;
        drop(self.pending.drain(..at_or_before));
        Ok(Quantum::Applied(applied))
    }

    /// The writer independently checks that each member has a complete trace.
    /// An unordered but audible final prefix may be retained, never sealed.
    pub(super) fn completed(&self) -> Option<DraftClock> {
        if self.poisoned
            || self.open_cohort.is_some()
            || !self.members[..self.group.ids().len()].iter().all(|member| {
                member
                    .latest
                    .is_some_and(|row| row.application == GroupApplication::Restored)
            })
        {
            return None;
        }
        self.latest.map(|row| row.clock)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use hex_audio::plugin_effects::EffectBinding;
    use hex_audio::prepared::AppliedAudioSource;
    use std::num::NonZeroU64;

    #[test]
    fn an_extra_member_interrupts_the_take_but_keeps_the_exact_main_prefix() {
        let target = Target {
            source: AppliedAudioSource {
                context: hex_engine::AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 2,
                },
                audio_revision: 3,
                output_identity: 4,
            },
            binding: EffectBinding {
                root: 4,
                instance: 5,
                graph: 6,
                epoch: 7,
            },
            transport_epoch: 8,
            scope: NonZeroU64::MIN,
            quantum_frames: 8,
        };
        let state = hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new([9; 16]).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [6; 32],
                    info_sha256: [7; 32],
                },
            },
            component: Some(&[]),
            controller: Some(&[]),
        })
        .unwrap();
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let hex_project::Created::Device(device) = editor
            .execute(hex_project::Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("device");
        };
        let mut correlation = GroupCorrelation::new(
            target,
            Vst3GroupTarget::new(device, [9; 16], &[0, 11]).unwrap(),
        );
        let edit = |kind, sequence, gesture, id, normalized, first_frame| ParameterEdit {
            kind,
            sequence,
            gesture,
            id,
            normalized,
            first_frame,
        };
        for event in [
            edit(Kind::GroupBegin, 1, 1, 0, 0.0, 8),
            edit(Kind::Begin, 2, 2, 0, 0.0, 8),
            edit(Kind::Begin, 3, 3, 11, 0.0, 8),
            edit(Kind::GroupEnd, 4, 1, 0, 0.0, 8),
            edit(Kind::GroupBegin, 5, 5, 0, 0.0, 16),
            edit(Kind::Perform, 6, 2, 0, 0.75, 16),
            edit(Kind::Perform, 7, 3, 11, 0.5, 16),
            edit(Kind::GroupEnd, 8, 5, 0, 0.0, 16),
        ] {
            assert!(correlation.ordered(event));
        }
        let fact = |parameter, sequence, gesture, application| Fact {
            parameter,
            sequence,
            gesture,
            application,
        };
        let clock = |frame| DraftClock {
            performance_frame: frame,
            content_frame: frame + 80,
            loop_pass: 0,
        };
        let Quantum::Applied(rows) = correlation
            .confirmed_facts(
                16,
                clock(16),
                false,
                [
                    fact(0, 6, 2, GroupApplication::Value(0.75)),
                    fact(11, 7, 3, GroupApplication::Value(0.5)),
                ]
                .into_iter(),
            )
            .unwrap()
        else {
            panic!("actual paired rows");
        };
        assert_eq!(rows[0].unwrap().ordinal, 1);
        assert_eq!(rows[1].unwrap().ordinal, 2);

        assert!(correlation.ordered(edit(Kind::GroupBegin, 9, 9, 0, 0.0, 24)));
        assert!(correlation.ordered(edit(Kind::End, 10, 2, 0, 0.0, 24)));
        assert!(!correlation.ordered(edit(Kind::Begin, 11, 11, 23, 0.0, 24)));
        assert!(!correlation.ordered(edit(Kind::End, 12, 3, 11, 0.0, 24)));
        assert!(!correlation.ordered(edit(Kind::GroupEnd, 13, 9, 0, 0.0, 24)));
        let Quantum::Applied(rows) = correlation
            .confirmed_facts(
                32,
                clock(32),
                false,
                [
                    fact(0, 10, 2, GroupApplication::Restored),
                    fact(11, 12, 3, GroupApplication::Restored),
                ]
                .into_iter(),
            )
            .unwrap()
        else {
            panic!("exact original restoration prefix");
        };
        assert_eq!(rows[0].unwrap().ordinal, 3);
        assert_eq!(rows[1].unwrap().ordinal, 4);
        assert_eq!(correlation.completed(), None);
    }
}
