//! Canonical read inventory shared by device lookup, capacity and media checks.
use crate::{Device, DeviceId, Invalid, Project};

pub const MAX_SERIAL_DEVICE_DEPTH: usize = 8;

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

    pub(crate) fn validate_serial_units(&self) -> Result<(), Invalid> {
        validate_chain(self.main_devices().iter())?;
        for track in self.tracks() {
            validate_chain(track.devices().iter())?;
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
        // A partial removal/copy cannot orphan or silently drop the suffix.
        let root = editor.project().main_devices()[0].id();
        assert!(editor.execute(Command::RemoveDevice(root)).is_err());
        assert_eq!(editor.project().snapshot(), authored);
        assert!(
            editor
                .project()
                .capture_devices(
                    crate::DeviceChainOwner::Main,
                    &crate::DeviceSelectionIds::new(&[root]).unwrap()
                )
                .is_err()
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
