//! Control-side atomic device state/inventory replacement. Expected rows include
//! points, touch spans, read selection and class stamps. No provider mapping is
//! inferred: callers explicitly supply the complete new inventory. Ordinary
//! state edits retain their existing preservation/provider-change refusal law.
//! This owner uses neutral model/contract types and never prepares DSP or media.
use super::*;
use hex_lattice_contract::NativeDeviceState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceAutomationReplacement {
    expected: Vec<DeviceAutomation>,
    replacement: Vec<DeviceAutomation>,
}
impl DeviceAutomationReplacement {
    /// Validate both bounded inventories before queueing a control edit. The
    /// project owner also validates the aggregate budget at final publication.
    pub fn new(
        expected: Vec<DeviceAutomation>,
        replacement: Vec<DeviceAutomation>,
    ) -> Result<Self, Invalid> {
        for rows in [&expected, &replacement] {
            validate_device_automation(rows)?;
            let points = rows
                .iter()
                .try_fold(0usize, |count, row| count.checked_add(row.point_count()));
            if points.is_none_or(|count| count > MAX_DEVICE_AUTOMATION_POINTS) {
                return Err(Invalid("device automation point limit reached"));
            }
        }
        Ok(Self {
            expected,
            replacement,
        })
    }
    pub fn expected(&self) -> &[DeviceAutomation] {
        &self.expected
    }
    pub fn replacement(&self) -> &[DeviceAutomation] {
        &self.replacement
    }
}
impl Device {
    pub(crate) fn replace_state_and_automation(
        &mut self,
        state: NativeDeviceState,
        change: DeviceAutomationReplacement,
    ) -> Result<(), Invalid> {
        if self.automation() != change.expected {
            return Err(Invalid(
                "device automation changed; retry from its current state",
            ));
        }
        // Control-only bounded copy keeps this mutation atomic even before the
        // editor's prepare-before-commit transaction. Clearing only the private
        // candidate allows an explicit new-provider inventory, not reinterpretation
        // of the old one. Ordinary replacement and A/B/global-trim rules stay owned
        // by replace_state; final class validation checks the intended new rows.
        let mut candidate = self.clone();
        candidate.automation.clear();
        candidate.replace_state(state)?;
        candidate.automation = change.replacement;
        candidate.validate_automation_provider(candidate.state())?;
        *self = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, points, state};
    use super::*;
    use crate::{Command, DeviceVariant, Editor};
    use std::cell::Cell;
    fn setup() -> (Editor, DeviceId) {
        let (mut e, _, device) = fixture();
        for raw in [7, 8] {
            e.execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: ParameterId::from_static(raw),
                points: points(),
            })
            .unwrap();
        }
        (e, device)
    }
    fn changed_state() -> NativeDeviceState {
        NativeDeviceState::new(state().type_id().clone(), 1, vec![9, 8]).unwrap()
    }
    fn command(e: &Editor, device: DeviceId, rows: Vec<DeviceAutomation>) -> Command {
        let d = e.project().device(device).unwrap();
        Command::ReplaceDeviceStateWithIdentityReservation {
            automation: Some(
                DeviceAutomationReplacement::new(d.automation().to_vec(), rows).unwrap(),
            ),
            device,
            expected_state: d.state().clone(),
            expected_variant: d.active_variant(),
            state: changed_state(),
            expected_next_id: e.project().next_identity(),
            reserved_ids: 0,
        }
    }
    #[test]
    fn state_and_inventory_commit_once_and_undo_restores_both() {
        let (mut e, device) = setup();
        e.execute(Command::SpliceDeviceAutomationTouch {
            device,
            target: DeviceAutomationTarget::Parameter(ParameterId::from_static(8)),
            vst3_class: None,
            spans: vec![DeviceAutomationTouchSpan {
                start: BeatTime::ZERO,
                end: BeatTime::from_raw(17),
                steps: vec![DeviceAutomationTouchStep {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.4).unwrap(),
                }],
            }],
        })
        .unwrap();
        e.execute(Command::SetDeviceAutomationReadSelection {
            device,
            target: DeviceAutomationTarget::Parameter(ParameterId::from_static(8)),
            selection: AutomationReadSelection::Base,
        })
        .unwrap();
        e.execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::HostInputGain,
            points: points(),
        })
        .unwrap();
        let before = e.project().snapshot();
        let kept = e.project().device(device).unwrap().automation()[1..].to_vec();
        e.execute(command(&e, device, kept.clone())).unwrap();
        assert_eq!(
            e.project().device(device).unwrap().state(),
            &changed_state()
        );
        assert_eq!(e.project().device(device).unwrap().automation(), kept);
        let after = e.project().snapshot();
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), before);
        assert!(e.redo());
        assert_eq!(e.project().snapshot(), after);
    }
    #[test]
    fn inventory_only_edit_prepares_the_new_rows_without_changing_provider_bytes() {
        let (mut e, device) = setup();
        let before = e.project().snapshot();
        let current = e.project().device(device).unwrap().state().clone();
        let mut cmd = command(&e, device, vec![]);
        if let Command::ReplaceDeviceStateWithIdentityReservation { state, .. } = &mut cmd {
            *state = current.clone();
        }
        let calls = Cell::new(0);
        e.execute_prepared(cmd, |candidate| {
            calls.set(calls.get() + 1);
            assert_eq!(candidate.device(device).unwrap().state(), &current);
            assert!(candidate.device(device).unwrap().automation().is_empty());
            Ok::<_, Invalid>(())
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), before);
    }
    #[test]
    fn stale_inventory_and_existing_state_variant_allocator_guards_refuse_before_preparation() {
        let (mut e, device) = setup();
        let captured = command(&e, device, vec![]);
        e.execute(Command::SetDeviceAutomationReadSelection {
            device,
            target: DeviceAutomationTarget::Parameter(ParameterId::from_static(7)),
            selection: AutomationReadSelection::Base,
        })
        .unwrap();
        let before = e.project().snapshot();
        let calls = Cell::new(0);
        assert!(
            e.execute_prepared(captured, |_| {
                calls.set(calls.get() + 1);
                Ok::<_, Invalid>(())
            })
            .is_err()
        );
        assert_eq!(calls.get(), 0);
        assert_eq!(e.project().snapshot(), before);
        for guard in 0..3 {
            let mut cmd = command(&e, device, vec![]);
            if let Command::ReplaceDeviceStateWithIdentityReservation {
                expected_state,
                expected_variant,
                expected_next_id,
                ..
            } = &mut cmd
            {
                match guard {
                    0 => *expected_state = changed_state(),
                    1 => *expected_variant = DeviceVariant::B,
                    _ => *expected_next_id += 1,
                }
            }
            assert!(
                e.execute_prepared(cmd, |_| {
                    calls.set(calls.get() + 1);
                    Ok::<_, Invalid>(())
                })
                .is_err()
            );
            assert_eq!(calls.get(), 0);
            assert_eq!(e.project().snapshot(), before);
        }
    }
    #[test]
    fn failed_preparation_and_new_provider_class_leave_state_inventory_and_history_intact() {
        let (mut e, device) = setup();
        let before = e.project().snapshot();
        assert!(
            e.execute_prepared(command(&e, device, vec![]), |_| Err::<(), _>(Invalid(
                "preparation refused"
            )))
            .is_err()
        );
        assert_eq!(e.project().snapshot(), before);
        let mut foreign = e.project().device(device).unwrap().automation()[0].clone();
        foreign.target = DeviceAutomationTarget::Vst3 { parameter: 0 };
        foreign.vst3_class = Some([1; 16]);
        let mut direct = e.project().device(device).unwrap().clone();
        let unchanged = direct.clone();
        let change =
            DeviceAutomationReplacement::new(direct.automation().to_vec(), vec![foreign.clone()])
                .unwrap();
        assert!(
            direct
                .replace_state_and_automation(changed_state(), change)
                .is_err()
        );
        assert_eq!(direct, unchanged);
        assert!(e.execute(command(&e, device, vec![foreign])).is_err());
        assert_eq!(e.project().snapshot(), before);
        assert!(e.undo());
        assert_eq!(e.project().device(device).unwrap().automation().len(), 1);
    }
    #[test]
    fn explicit_provider_mapping_is_distinct_from_ordinary_state_replacement() {
        let (mut e, device) = setup();
        let before = e.project().snapshot();
        let next = NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::from_static("test.mapped"),
            1,
            vec![3],
        )
        .unwrap();
        assert!(
            e.execute(Command::ReplaceDeviceState {
                device,
                state: next.clone()
            })
            .is_err()
        );
        let mut rows = e.project().device(device).unwrap().automation().to_vec();
        rows[0].target = DeviceAutomationTarget::Parameter(ParameterId::from_static(9));
        rows[1].target = DeviceAutomationTarget::Parameter(ParameterId::from_static(10));
        let mut cmd = command(&e, device, rows.clone());
        if let Command::ReplaceDeviceStateWithIdentityReservation { state, .. } = &mut cmd {
            *state = next.clone();
        }
        e.execute(cmd).unwrap();
        assert_eq!(e.project().device(device).unwrap().state(), &next);
        assert_eq!(e.project().device(device).unwrap().automation(), rows);
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), before);
    }
    #[test]
    fn replacement_constructor_rejects_duplicate_targets_and_total_point_overflow() {
        let (e, device) = setup();
        let row = e.project().device(device).unwrap().automation()[0].clone();
        assert!(DeviceAutomationReplacement::new(vec![], vec![row.clone(), row]).is_err());
        let points: Vec<_> = (0..1024)
            .map(|beat| AutomationPoint {
                beat: BeatTime::from_raw(beat),
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Step,
            })
            .collect();
        let rows = (1..=65)
            .map(|id| DeviceAutomation {
                read_selection: AutomationReadSelection::Authored,
                target: DeviceAutomationTarget::Parameter(ParameterId::from_static(id)),
                vst3_class: None,
                points: points.clone(),
                touch: vec![],
            })
            .collect();
        assert!(DeviceAutomationReplacement::new(vec![], rows).is_err());
    }
}
