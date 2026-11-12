//! Control-side effective power over a bounded canonical effect preorder.
//! Uses a fixed ancestor stack; authored state and callback ownership never change.
//! Model validation owns identities and membership. A missing active ancestor
//! still refuses here so a sliced or misordered chain cannot silently lose power.
use crate::Invalid;
use hex_project::{Device, DeviceId, MAX_SERIAL_DEVICE_DEPTH};

pub(crate) struct EffectPower<'a> {
    devices: &'a [Device],
    cursor: usize,
    ancestors: [Option<(DeviceId, bool)>; MAX_SERIAL_DEVICE_DEPTH],
    depth: usize,
}
impl<'a> EffectPower<'a> {
    pub(crate) fn new(devices: &'a [Device]) -> Result<Self, Invalid> {
        if devices.len() > hex_project::MAX_DEVICES_PER_TRACK {
            return Err(Invalid("effect chain exceeds its device limit"));
        }
        Ok(Self {
            devices,
            cursor: 0,
            ancestors: [None; MAX_SERIAL_DEVICE_DEPTH],
            depth: 0,
        })
    }

    /// Seed the suffix with the omitted instrument. A root sibling still
    /// resets inherited power, just as in the full authored preorder.
    pub(crate) fn after_instrument(
        devices: &'a [Device],
        instrument: &Device,
    ) -> Result<Self, Invalid> {
        let mut power = Self::new(devices)?;
        power.ancestors[0] = Some((instrument.id(), instrument.bypassed()));
        power.depth = 1;
        Ok(power)
    }

    pub(crate) fn remaining(&self) -> usize {
        self.devices.len() - self.cursor
    }
}
impl<'a> Iterator for EffectPower<'a> {
    type Item = Result<(&'a Device, bool), Invalid>;
    fn next(&mut self) -> Option<Self::Item> {
        let device = self.devices.get(self.cursor)?;
        self.cursor += 1;
        let parent = match device.parent() {
            None => Some((0, false)),
            Some(parent) => {
                self.ancestors[..self.depth]
                    .iter()
                    .enumerate()
                    .find_map(|(index, ancestor)| match ancestor {
                        Some((id, bypassed)) if *id == parent => Some((index + 1, *bypassed)),
                        _ => None,
                    })
            }
        };
        let Some((depth, inherited)) = parent else {
            self.cursor = self.devices.len();
            return Some(Err(Invalid(
                "effect parent is outside the active ancestor chain",
            )));
        };
        let Some(slot) = self.ancestors.get_mut(depth) else {
            self.cursor = self.devices.len();
            return Some(Err(Invalid("effect unit exceeds its depth limit")));
        };
        let bypassed = inherited || device.bypassed();
        *slot = Some((device.id(), bypassed));
        self.depth = depth + 1;
        Some(Ok((device, bypassed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{Command, Editor, Project};

    fn fixture() -> Project {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        for _ in 0..7 {
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
                        .unwrap(),
                })
                .unwrap();
        }
        let mut snapshot = editor.project().snapshot();
        let chain = &mut snapshot.main_devices;
        chain[1].parent = Some(chain[0].id);
        chain[2].parent = Some(chain[1].id);
        chain[3].parent = Some(chain[0].id);
        chain[5].parent = Some(chain[4].id);
        chain[1].bypassed = true;
        chain[4].bypassed = true;
        Project::from_snapshot(snapshot).unwrap()
    }
    fn flags(devices: &[Device]) -> Result<Vec<bool>, Invalid> {
        EffectPower::new(devices)?
            .map(|entry| entry.map(|(_, bypassed)| bypassed))
            .collect()
    }
    #[test]
    fn ancestor_power_stops_at_siblings_and_reenable_preserves_child_switches() {
        let project = fixture();
        let before = project.snapshot();
        assert_eq!(
            flags(project.main_devices()).unwrap(),
            [false, true, true, false, true, true, false]
        );
        let mut editor = Editor::new(project, 8).unwrap();
        let root = editor.project().main_devices()[0].id();
        editor
            .execute(Command::SetDeviceBypass {
                device: root,
                bypassed: true,
            })
            .unwrap();
        assert_eq!(
            flags(editor.project().main_devices()).unwrap(),
            [true, true, true, true, true, true, false]
        );
        editor
            .execute(Command::SetDeviceBypass {
                device: root,
                bypassed: false,
            })
            .unwrap();
        assert_eq!(editor.project().snapshot(), before);
        assert_eq!(
            flags(editor.project().main_devices()).unwrap(),
            [false, true, true, false, true, true, false]
        );
    }
    #[test]
    fn flat_capacity_control_and_missing_ancestor_refuse_without_partial_continuation() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        for _ in 0..hex_project::MAX_DEVICES_PER_TRACK {
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
                        .unwrap(),
                })
                .unwrap();
        }
        assert_eq!(
            flags(editor.project().main_devices()).unwrap(),
            vec![false; 64]
        );
        let mut deep = editor.project().snapshot();
        for i in 1..MAX_SERIAL_DEVICE_DEPTH {
            deep.main_devices[i].parent = Some(deep.main_devices[i - 1].id);
        }
        let deep = Project::from_snapshot(deep).unwrap();
        assert_eq!(flags(deep.main_devices()).unwrap(), vec![false; 64]);
        let mut oversized = editor.project().main_devices().to_vec();
        oversized.push(oversized[0].clone());
        assert!(EffectPower::new(&oversized).is_err());
        let project = fixture();
        let mut missing = EffectPower::new(&project.main_devices()[1..]).unwrap();
        assert!(missing.next().unwrap().is_err());
        assert!(missing.next().is_none());
        let reordered = vec![
            project.main_devices()[1].clone(),
            project.main_devices()[0].clone(),
        ];
        assert!(flags(&reordered).is_err());
    }
}
