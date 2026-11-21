//! Authored admission for the direct Resident hosted effect reorder.
use crate::service::{ResidentHostedTrackEffectReorder, ResidentRuntimeLoop};
use hex_project::{Project, TrackId};

pub(super) fn preparation(
    before: &Project,
    after: &Project,
    track: TrackId,
    predecessor: ResidentRuntimeLoop,
) -> Option<ResidentHostedTrackEffectReorder> {
    // The Editor already staged a typed ReorderDevices command. The
    // installed playback owner must still prove runtime routes and clocks.
    let [first, second] = before.tracks() else {
        return None;
    };
    if !before.main_devices().is_empty()
        || !before.return_tracks().is_empty()
        || !before.native_sources().is_empty()
        || before.loop_region().is_some()
        || before.has_gain_automation()
        || [first, second].iter().any(|row| {
            row.is_group()
                || row.parent_group().is_some()
                || row.clips().len() != 1
                || !row.midi_clips().is_empty()
                || row.instrument().is_some()
                || row.source_id().is_some()
                || !row.sends().is_empty()
                || (row.id() != track && !row.devices().is_empty())
        })
    {
        return None;
    }
    let [first, second] = before.track(track)?.devices() else {
        return None;
    };
    let [next_first, next_second] = after.track(track)?.devices() else {
        return None;
    };
    if next_first != second
        || next_second != first
        || [first, second].iter().any(|device| {
            device.parent().is_some()
                || device.bypassed()
                || !device.automation().is_empty()
                || !device.modulation().is_empty()
        })
    {
        return None;
    }
    let is_hosted =
        |device: &hex_project::Device| device.state().type_id().as_str() == Ok("hex.vst3");
    let hosted = match (is_hosted(first), is_hosted(second)) {
        (true, false) => first,
        (false, true) => second,
        _ => return None,
    };
    Some(ResidentHostedTrackEffectReorder {
        track,
        device: hosted.id(),
        predecessor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ResidentSourceOrigin;
    use hex_project::{Command, Created, DeviceChainOwner, DeviceSelectionIds, Editor};

    #[test]
    fn selects_only_the_exact_enabled_hosted_permutation() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut import = |name: &str| {
            let Created::Imported { track, .. } = editor
                .execute(Command::ImportAudioTrack {
                    name: name.into(),
                    path: format!("memory/{name}.wav").into(),
                    sample_rate: 48_000,
                    frames: 4096,
                })
                .unwrap()
            else {
                panic!("direct audio track");
            };
            track
        };
        let track = import("edited");
        let dry = import("dry");
        let native = hex_engine::default_native_device_state(
            hex_lattice_contract::DeviceTypeId::from_static("hex.delay"),
        )
        .unwrap();
        let hosted = hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [2; 32],
                    info_sha256: [3; 32],
                },
            },
            component: Some(b"preset"),
            controller: None,
        })
        .unwrap();
        for state in [native, hosted] {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state,
                })
                .unwrap();
        }
        let before = editor.project();
        let [native, hosted] = before.track(track).unwrap().devices() else {
            panic!("mixed device pair");
        };
        let native = native.id();
        let hosted = hosted.id();
        let predecessor = ResidentRuntimeLoop {
            output_identity: 1,
            audio_revision: 3,
            source_context: Some(hex_engine::AudioSourceContext {
                project_generation: 1,
                editor_revision: 2,
            }),
            source_origin: ResidentSourceOrigin::CanonicalCandidate,
            section_loop: None,
        };
        let edit = editor
            .stage_edit(Command::ReorderDevices {
                owner: DeviceChainOwner::Track(track),
                selected: DeviceSelectionIds::new(&[native]).unwrap(),
                before: None,
            })
            .unwrap();
        let before = editor.project();
        assert_eq!(
            preparation(before, edit.project(), track, predecessor),
            Some(ResidentHostedTrackEffectReorder {
                track,
                device: hosted,
                predecessor
            }),
        );
        assert!(preparation(before, before, track, predecessor).is_none());
        assert!(preparation(before, edit.project(), dry, predecessor).is_none());
    }
}
