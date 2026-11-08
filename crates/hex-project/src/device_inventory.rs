//! Canonical read inventory shared by device lookup, capacity and media checks.
use crate::{Device, DeviceId, Project};

impl Project {
    /// Each authored device root exactly once, including unbound retained sources.
    ///
    /// Track instruments are immutable views of retained sources, not additional
    /// owners. Visit track-local effects, Returns, Main, then retained sources in
    /// inventory order; each source retains its own signal order. This is a
    /// control-side inventory, not a playback schedule or a mutable binding view.
    pub fn device_roots(&self) -> impl Iterator<Item = &Device> {
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
        self.device_roots().find(|device| device.id() == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, Editor, default_instrument_state};

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
