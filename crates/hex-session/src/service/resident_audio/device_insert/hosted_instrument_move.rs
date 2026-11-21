//! Only a staged single-device move can lend its canonical route change.
use super::{ResidentHostedInstrumentMove, ResidentRuntimeLoop};
use crate::service::ResidentHostedTrackEffectMove;
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

    pub(super) fn track_effect_preparation(
        self,
        before: &Project,
        after: &Project,
        predecessor: ResidentRuntimeLoop,
    ) -> Option<ResidentHostedTrackEffectMove> {
        self.exact_track_effect_move(before, after)?;
        Some(ResidentHostedTrackEffectMove {
            donor: self.donor,
            receiver: self.receiver,
            device: self.device,
            predecessor,
        })
    }

    pub(super) fn exact_track_effect_move(self, before: &Project, after: &Project) -> Option<()> {
        let direct_pair = |project: &Project| {
            project.tracks().len() == 2
                && project.main_devices().is_empty()
                && project.return_tracks().is_empty()
                && project.native_sources().is_empty()
                && project.loop_region().is_none()
                && !project.has_gain_automation()
                && project.tracks().iter().all(|track| {
                    !track.is_group()
                        && track.parent_group().is_none()
                        && track.clips().len() == 1
                        && track.midi_clips().is_empty()
                        && track.source_id().is_none()
                        && track.instrument().is_none()
                        && track.sends().is_empty()
                })
        };
        if !direct_pair(before)
            || !direct_pair(after)
            || before.sample_rate() != after.sample_rate()
            || !before
                .tracks()
                .iter()
                .zip(after.tracks())
                .all(|(old, new)| old.id() == new.id() && old.clips() == new.clips())
        {
            return None;
        }
        let [original] = before.track(self.donor)?.devices() else {
            return None;
        };
        let [moved] = after.track(self.receiver)?.devices() else {
            return None;
        };
        (original.id() == self.device
            && original == moved
            && matches!(original.state().type_id().as_str(), Ok("hex.vst3"))
            && original.parent().is_none()
            && !original.bypassed()
            && original.automation().is_empty()
            && original.modulation().is_empty()
            && before.track(self.receiver)?.devices().is_empty()
            && after.track(self.donor)?.devices().is_empty())
        .then_some(())
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

    fn hosted_state() -> hex_lattice_contract::NativeDeviceState {
        hex_plugin_state::encode(hex_plugin_state::State {
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
        .unwrap()
    }

    fn installed() -> ResidentRuntimeLoop {
        ResidentRuntimeLoop {
            output_identity: 7,
            audio_revision: 8,
            source_context: Some(hex_engine::AudioSourceContext {
                project_generation: 2,
                editor_revision: 3,
            }),
            source_origin: ResidentSourceOrigin::CanonicalCandidate,
            section_loop: None,
        }
    }

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
        let Created::Device(device) = editor
            .execute(Command::InsertInstrument {
                track: donor,
                state: hosted_state(),
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
        let predecessor = installed();
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

    #[test]
    fn track_effect_move_witness_requires_the_exact_two_direct_audio_rows() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut add = |name: &str| {
            let Created::Imported { track, .. } = editor
                .execute(Command::ImportAudioTrack {
                    name: name.into(),
                    path: format!("{name}.wav").into(),
                    sample_rate: 48_000,
                    frames: 1024,
                })
                .unwrap()
            else {
                panic!("audio track");
            };
            track
        };
        let donor = add("donor");
        let receiver = add("receiver");
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track: donor,
                before: None,
                state: hosted_state(),
            })
            .unwrap()
        else {
            panic!("hosted effect");
        };
        let movement = || Command::MoveTrackDevices {
            source: donor,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
            destination: receiver,
        };
        let intent = Intent::from_command(&movement()).unwrap();
        let staged = editor.stage_edit(movement()).unwrap();
        assert_eq!(
            intent.track_effect_preparation(editor.project(), staged.project(), installed()),
            Some(ResidentHostedTrackEffectMove {
                donor,
                receiver,
                device,
                predecessor: installed()
            }),
        );
        assert!(
            intent
                .preparation(editor.project(), staged.project(), installed())
                .is_none()
        );
        editor
            .execute(Command::AddTrack {
                name: "third".into(),
            })
            .unwrap();
        let staged = editor.stage_edit(movement()).unwrap();
        assert!(
            intent
                .track_effect_preparation(editor.project(), staged.project(), installed())
                .is_none()
        );
    }
}
