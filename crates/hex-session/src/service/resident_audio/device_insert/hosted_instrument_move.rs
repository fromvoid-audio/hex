//! Only the staged single-device command can lend its canonical route change.
use super::{ResidentHostedInstrumentMove, ResidentRuntimeLoop};
use hex_project::{Command, Device, DeviceId, Project, SourceId, Track, TrackId};

#[derive(Clone, Copy)]
pub(super) struct Intent {
    donor: TrackId,
    receiver: TrackId,
    device: DeviceId,
}

impl Intent {
    pub(super) fn from_command(command: &Command) -> Option<Self> {
        let Command::MoveTrackDevices {
            source,
            selected,
            destination,
        } = command
        else {
            return None;
        };
        let [device] = selected.ids() else {
            return None;
        };
        (source != destination).then_some(Self {
            donor: *source,
            receiver: *destination,
            device: *device,
        })
    }

    pub(super) fn preparation(
        self,
        before: &Project,
        after: &Project,
        predecessor: ResidentRuntimeLoop,
    ) -> Option<ResidentHostedInstrumentMove> {
        if before.sample_rate() != after.sample_rate() {
            return None;
        }
        let donor = direct_track(before, self.donor)?;
        let emptied = direct_track(after, self.donor)?;
        let empty = direct_track(before, self.receiver)?;
        let receiver = direct_track(after, self.receiver)?;
        if empty.instrument().is_some()
            || empty.source_id().is_some()
            || emptied.instrument().is_some()
            || emptied.source_id().is_some()
        {
            return None;
        }
        let (original, source) = solitary_instrument(before, donor)?;
        let (moved, moved_source) = solitary_instrument(after, receiver)?;
        (original.id() == self.device && original == moved && source == moved_source).then_some(
            ResidentHostedInstrumentMove {
                donor: self.donor,
                receiver: self.receiver,
                device: self.device,
                source,
                predecessor,
            },
        )
    }
}

fn direct_track(project: &Project, id: TrackId) -> Option<&Track> {
    project.track(id).filter(|track| {
        !track.is_group() && track.parent_group().is_none() && track.devices().is_empty()
    })
}

fn solitary_instrument<'a>(project: &Project, track: &'a Track) -> Option<(&'a Device, SourceId)> {
    let device = track.instrument()?;
    let id = track.source_id()?;
    let source = project.native_source(id)?;
    (matches!(device.state().type_id().as_str(), Ok("hex.vst3"))
        && !device.bypassed()
        && device.automation().is_empty()
        && device.modulation().is_empty()
        && source.receiver() == Some(track.id())
        && source.devices().count() == 1
        && project.source_contributors(id).count() == 1)
        .then_some((device, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ResidentSourceOrigin;
    use hex_project::{Created, DeviceSelectionIds, Editor};

    #[test]
    fn witnesses_only_the_exact_enabled_direct_hosted_move() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut add = |name: &str| {
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack { name: name.into() })
                .unwrap()
            else {
                panic!("MIDI track");
            };
            track
        };
        let donor = add("donor");
        let receiver = add("receiver");
        let other = add("other");
        let state = hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [2; 32],
                    info_sha256: [3; 32],
                },
            },
            component: Some(b"original"),
            controller: None,
        })
        .unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertInstrument {
                track: donor,
                state,
            })
            .unwrap()
        else {
            panic!("hosted instrument");
        };
        let move_to = |destination| Command::MoveTrackDevices {
            source: donor,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
            destination,
        };
        let command = move_to(receiver);
        let intent = Intent::from_command(&command).unwrap();
        let predecessor = ResidentRuntimeLoop {
            output_identity: 7,
            audio_revision: 8,
            source_context: Some(hex_engine::AudioSourceContext {
                project_generation: 2,
                editor_revision: 3,
            }),
            source_origin: ResidentSourceOrigin::CanonicalCandidate,
            section_loop: None,
        };
        let edit = editor.stage_edit(command).unwrap();
        let source = editor.project().track(donor).unwrap().source_id().unwrap();
        assert_eq!(
            intent.preparation(editor.project(), edit.project(), predecessor),
            Some(ResidentHostedInstrumentMove {
                donor,
                receiver,
                device,
                source,
                predecessor
            })
        );
        let wrong = editor.stage_edit(move_to(other)).unwrap();
        assert!(
            intent
                .preparation(editor.project(), wrong.project(), predecessor)
                .is_none()
        );
        assert!(Intent::from_command(&Command::RemoveDevice(device)).is_none());
        editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        let bypassed = editor.stage_edit(move_to(receiver)).unwrap();
        assert!(
            intent
                .preparation(editor.project(), bypassed.project(), predecessor)
                .is_none()
        );
    }
}
