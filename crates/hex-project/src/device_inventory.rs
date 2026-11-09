//! Canonical read inventory shared by device lookup, capacity and media checks.
use crate::{
    Device, DeviceChainOwner, DeviceId, DeviceUnitClass, Invalid, NativeSource, Project, ReturnId,
    SourceId, TrackId,
};

pub const MAX_SERIAL_DEVICE_DEPTH: usize = 8;

/// Actual authored owner. A track showing a shared source does not own it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceOwner {
    Track(TrackId),
    Return(ReturnId),
    Main,
    Source(SourceId),
}

#[derive(Clone, Copy, Debug)]
pub struct DeviceLocation<'a> {
    pub device: &'a Device,
    pub owner: DeviceOwner,
    pub class: DeviceUnitClass,
}

fn find_effect(devices: &[Device], owner: DeviceOwner, id: DeviceId) -> Option<DeviceLocation<'_>> {
    let device = devices.iter().find(|device| device.id() == id)?;
    Some(DeviceLocation {
        device,
        owner,
        class: DeviceUnitClass::Effect,
    })
}

fn find_source(source: &NativeSource, id: DeviceId) -> Option<DeviceLocation<'_>> {
    let device = source.devices().find(|device| device.id() == id)?;
    Some(DeviceLocation {
        device,
        owner: DeviceOwner::Source(source.id()),
        class: if source.instrument().id() == id {
            DeviceUnitClass::Instrument
        } else {
            DeviceUnitClass::Effect
        },
    })
}

/// Contiguous subtree in an already validated, bounded canonical owner chain.
/// Parent IDs always name earlier nodes; stop when a node leaves this subtree.
pub(crate) fn serial_unit_span(
    devices: &[Device],
    start: usize,
) -> Result<std::ops::Range<usize>, Invalid> {
    devices
        .get(start)
        .ok_or(Invalid("device position is out of bounds"))?;
    let mut end = start + 1;
    while let Some(device) = devices.get(end) {
        if !device.parent().is_some_and(|parent| {
            devices[start..end]
                .iter()
                .any(|ancestor| ancestor.id() == parent)
        }) {
            break;
        }
        end += 1;
    }
    Ok(start..end)
}

impl Project {
    /// Each authored device root exactly once, including unbound retained sources.
    ///
    /// Track instruments are immutable views of retained sources, not additional
    /// owners. Visit track-local effects, Returns, Main, then retained sources in
    /// inventory order; each source retains its own signal order. This is a
    /// control-side inventory, not a playback schedule or a mutable binding view.
    pub fn device_roots(&self) -> impl Iterator<Item = &Device> {
        self.devices().filter(|device| device.parent().is_none())
    }

    /// Every canonical node once, in owner order and serial preorder per owner.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.tracks()
            .iter()
            .flat_map(|track| track.devices())
            .chain(
                self.return_tracks()
                    .iter()
                    .flat_map(|track| track.devices()),
            )
            .chain(self.main_devices())
            .chain(
                self.native_sources()
                    .iter()
                    .flat_map(|source| source.devices()),
            )
    }

    pub fn device(&self, id: DeviceId) -> Option<&Device> {
        self.devices().find(|device| device.id() == id)
    }

    /// Resolve any existing canonical device and its native slot class.
    pub fn device_location(&self, id: DeviceId) -> Option<DeviceLocation<'_>> {
        self.tracks()
            .iter()
            .find_map(|track| find_effect(track.devices(), DeviceOwner::Track(track.id()), id))
            .or_else(|| {
                self.return_tracks().iter().find_map(|track| {
                    find_effect(track.devices(), DeviceOwner::Return(track.id()), id)
                })
            })
            .or_else(|| find_effect(self.main_devices(), DeviceOwner::Main, id))
            .or_else(|| {
                self.native_sources()
                    .iter()
                    .find_map(|source| find_source(source, id))
            })
    }

    /// Resolve membership in one visible chain, including the track's current
    /// source binding. The returned owner is still the canonical source.
    pub fn device_in_chain(
        &self,
        chain: DeviceChainOwner,
        id: DeviceId,
    ) -> Option<DeviceLocation<'_>> {
        match chain {
            DeviceChainOwner::Track(owner) => {
                let track = self.track(owner)?;
                find_effect(track.devices(), DeviceOwner::Track(owner), id).or_else(|| {
                    let source = self.native_source(track.source_id()?)?;
                    find_source(source, id)
                })
            }
            DeviceChainOwner::Return(owner) => {
                let track = self
                    .return_tracks()
                    .iter()
                    .find(|track| track.id() == owner)?;
                find_effect(track.devices(), DeviceOwner::Return(owner), id)
            }
            DeviceChainOwner::Main => find_effect(self.main_devices(), DeviceOwner::Main, id),
        }
    }

    pub(crate) fn validate_serial_units(&self) -> Result<(), Invalid> {
        validate_chain(self.main_devices().iter())?;
        for track in self.tracks() {
            if let Some(position) = track.instrument_position() {
                // The retained instrument interrupts local signal order and is
                // a different canonical owner, never a local unit ancestor.
                let (before, after) = track
                    .devices()
                    .split_at_checked(position)
                    .ok_or(Invalid("track instrument position is out of bounds"))?;
                validate_chain(before.iter())?;
                validate_chain(after.iter())?;
            } else {
                validate_chain(track.devices().iter())?;
            }
        }
        for track in self.return_tracks() {
            validate_chain(track.devices().iter())?;
        }
        for source in self.native_sources() {
            if source.instrument().parent().is_some() {
                return Err(Invalid("source instrument must be a serial root"));
            }
            validate_chain(source.devices())?;
        }
        Ok(())
    }
}

fn validate_chain<'a>(devices: impl Iterator<Item = &'a Device>) -> Result<(), Invalid> {
    let mut ancestors = [None; MAX_SERIAL_DEVICE_DEPTH];
    let mut depth = 0;
    for device in devices {
        let next = match device.parent() {
            None => 0,
            Some(parent) => ancestors[..depth]
                .iter()
                .position(|ancestor| *ancestor == Some(parent))
                .map(|index| index + 1)
                .ok_or(Invalid(
                    "serial parent must precede its child in the same unit",
                ))?,
        };
        let slot = ancestors
            .get_mut(next)
            .ok_or(Invalid("serial device depth limit reached"))?;
        *slot = Some(device.id());
        depth = next + 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, Editor, default_instrument_state};

    #[test]
    fn locations_keep_nested_owners_and_resolve_only_current_track_bindings() {
        let state = default_instrument_state().unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut add_track = |name: &str| {
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track")
            };
            track
        };
        let track = add_track("one");
        let other = add_track("two");
        let Created::NativeSource(source) = editor
            .execute(Command::CreateNativeSource {
                name: "shared".into(),
                devices: vec![state.clone()],
                instrument_index: 0,
            })
            .unwrap()
        else {
            panic!("source")
        };
        let instrument = editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .id();
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        let mut add = |command| {
            let Created::Device(device) = editor.execute(command).unwrap() else {
                panic!("device")
            };
            device
        };
        let local = add(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        });
        let main = add(Command::InsertMainDevice {
            before: None,
            state: state.clone(),
        });
        let ret = add(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: state.clone(),
        });
        let children = [local, main, ret, instrument].map(|parent| {
            add(Command::InsertNestedDevice {
                parent,
                state: state.clone(),
            })
        });
        for owner in [track, other] {
            editor
                .execute(Command::BindTrackSource {
                    track: owner,
                    source: Some(source),
                })
                .unwrap();
        }
        let project = editor.project();
        assert_eq!(
            project.device_location(instrument).unwrap().class,
            DeviceUnitClass::Instrument
        );
        for (child, parent, chain, owner) in [
            (
                children[0],
                local,
                DeviceChainOwner::Track(track),
                DeviceOwner::Track(track),
            ),
            (children[1], main, DeviceChainOwner::Main, DeviceOwner::Main),
            (
                children[2],
                ret,
                DeviceChainOwner::Return(bus),
                DeviceOwner::Return(bus),
            ),
            (
                children[3],
                instrument,
                DeviceChainOwner::Track(track),
                DeviceOwner::Source(source),
            ),
        ] {
            for location in [
                project.device_location(child),
                project.device_in_chain(chain, child),
            ] {
                let location = location.unwrap();
                assert_eq!(location.owner, owner);
                assert_eq!(location.class, DeviceUnitClass::Effect);
                assert_eq!(location.device.parent(), Some(parent));
            }
        }
        let other_chain = DeviceChainOwner::Track(other);
        assert!(project.device_in_chain(other_chain, children[0]).is_none());
        assert_eq!(
            project
                .device_in_chain(other_chain, children[3])
                .unwrap()
                .owner,
            DeviceOwner::Source(source)
        );
        editor
            .execute(Command::BindTrackSource {
                track: other,
                source: None,
            })
            .unwrap();
        assert!(
            editor
                .project()
                .device_in_chain(other_chain, children[3])
                .is_none()
        );
        assert!(editor.project().device_location(children[3]).is_some());
    }

    #[test]
    fn local_units_cannot_cross_the_track_instrument_slot() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(Command::InsertInstrument {
                track,
                state: default_instrument_state().unwrap(),
            })
            .unwrap();
        for _ in 0..2 {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: default_instrument_state().unwrap(),
                })
                .unwrap();
        }
        let snapshot = editor.project().snapshot();
        for position in 0..=2 {
            let mut candidate = snapshot.clone();
            candidate.tracks[0].instrument_position = Some(position);
            assert!(
                Project::from_snapshot(candidate.clone()).is_ok(),
                "flat control at {position}"
            );
            candidate.tracks[0].devices[1].parent = Some(candidate.tracks[0].devices[0].id);
            assert_eq!(
                Project::from_snapshot(candidate).is_ok(),
                position != 1,
                "unit at {position}"
            );
        }
        let mut malformed = snapshot;
        malformed.tracks[0].instrument_position = Some(3);
        assert!(Project::from_snapshot(malformed).is_err());
    }

    #[test]
    fn serial_membership_is_bounded_preorder_and_preserved_by_scalar_history() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        for _ in 0..9 {
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: default_instrument_state().unwrap(),
                })
                .unwrap();
        }
        let mut snapshot = editor.project().snapshot();
        for i in 1..8 {
            snapshot.main_devices[i].parent = Some(snapshot.main_devices[i - 1].id);
        }
        let project = Project::from_snapshot(snapshot.clone()).unwrap();
        assert_eq!(project.devices().count(), 9);
        assert_eq!(project.device_roots().count(), 2);
        assert_eq!(crate::returns::device_count(&project).unwrap(), 9);
        let child = project.main_devices()[7].id();
        let mut editor = Editor::new(project, 8).unwrap();
        let before = editor.project().snapshot();
        editor
            .execute(Command::SetDeviceBypass {
                device: child,
                bypassed: true,
            })
            .unwrap();
        assert_eq!(
            editor
                .project()
                .device(child)
                .unwrap()
                .parent()
                .unwrap()
                .value(),
            before.main_devices[6].id
        );
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        let authored = editor.project().snapshot();
        // Removal and capture both own the whole subtree.
        let root = editor.project().main_devices()[0].id();
        editor.execute(Command::RemoveDevice(root)).unwrap();
        assert_eq!(editor.project().main_devices().len(), 1);
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), authored);
        assert!(
            editor
                .project()
                .capture_devices(
                    crate::DeviceChainOwner::Main,
                    &crate::DeviceSelectionIds::new(&[root]).unwrap()
                )
                .is_ok()
        );

        let mut bad = snapshot.clone();
        bad.main_devices[8].parent = Some(bad.main_devices[7].id);
        assert!(Project::from_snapshot(bad).is_err(), "depth nine");
        for parent in [0, u64::MAX, snapshot.main_devices[8].id] {
            let mut bad = snapshot.clone();
            bad.main_devices[1].parent = Some(parent);
            assert!(
                Project::from_snapshot(bad).is_err(),
                "missing, self/future parent"
            );
        }
        let mut bad = snapshot;
        bad.main_devices[2].parent = None;
        bad.main_devices[3].parent = Some(bad.main_devices[0].id);
        assert!(
            Project::from_snapshot(bad).is_err(),
            "a closed unit cannot resume"
        );
    }

    #[test]
    fn canonical_roots_include_all_owners_and_count_shared_sources_once() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let state = default_instrument_state().unwrap();
        let mut tracks = Vec::new();
        for name in ["receiver", "contributor"] {
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track fixture")
            };
            tracks.push(track);
        }
        let Created::NativeSource(shared) = editor
            .execute(Command::CreateNativeSource {
                name: "shared".into(),
                devices: vec![state.clone()],
                instrument_index: 0,
            })
            .unwrap()
        else {
            panic!("source fixture")
        };
        for &track in &tracks {
            editor
                .execute(Command::BindTrackSource {
                    track,
                    source: Some(shared),
                })
                .unwrap();
        }
        editor
            .execute(Command::InsertDevice {
                track: tracks[0],
                before: None,
                state: state.clone(),
            })
            .unwrap();
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return fixture")
        };
        editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: state.clone(),
            })
            .unwrap();
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: state.clone(),
            })
            .unwrap();
        let Created::NativeSource(unbound) = editor
            .execute(Command::CreateNativeSource {
                name: "unbound chain".into(),
                devices: vec![state.clone(), state.clone(), state],
                instrument_index: 1,
            })
            .unwrap()
        else {
            panic!("unbound fixture")
        };
        let project = editor.project();
        let mut expected = vec![
            project.track(tracks[0]).unwrap().devices()[0].id(),
            project.return_tracks()[0].devices()[0].id(),
            project.main_devices()[0].id(),
            project.native_source(shared).unwrap().instrument().id(),
        ];
        expected.extend(
            project
                .native_source(unbound)
                .unwrap()
                .devices()
                .map(Device::id),
        );
        assert_eq!(
            project.device_roots().map(Device::id).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(crate::returns::device_count(project).unwrap(), 7);
        assert_eq!(
            project
                .automation_devices()
                .map(Device::id)
                .collect::<Vec<_>>(),
            expected
        );
        for device in project.device_roots() {
            assert!(std::ptr::eq(project.device(device.id()).unwrap(), device));
        }

        // Deleting every binding keeps the canonical source and its capacity.
        for track in tracks {
            editor.execute(Command::RemoveTrack(track)).unwrap();
        }
        assert_eq!(crate::returns::device_count(editor.project()).unwrap(), 6);
        assert!(editor.project().device(expected[3]).is_some());
        assert!(editor.undo());
        assert_eq!(crate::returns::device_count(editor.project()).unwrap(), 6);
        assert!(editor.undo());
        assert_eq!(
            editor
                .project()
                .device_roots()
                .map(Device::id)
                .collect::<Vec<_>>(),
            expected
        );
    }
}
