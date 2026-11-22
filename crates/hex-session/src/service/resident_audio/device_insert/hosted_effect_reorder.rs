//! Authored admission for the direct Resident hosted effect reorder.
use crate::service::{ResidentHostedTrackEffectReorder, ResidentRuntimeLoop};
use hex_project::{Project, TrackId};

pub(super) fn preparation(
    before: &Project,
    after: &Project,
    track: TrackId,
    predecessor: ResidentRuntimeLoop,
) -> Option<ResidentHostedTrackEffectReorder> {
    let device = direct_hosted_pair(before, after, track)?;
    Some(ResidentHostedTrackEffectReorder {
        track,
        device,
        predecessor,
    })
}

/// The caller has proved the whole normalized history differs only by order.
/// The Engine decides whether the native route can fit the running timing.
pub(super) fn supports_history(before: &Project, after: &Project, track: TrackId) -> bool {
    let Some(row) = before.track(track) else {
        return false;
    };
    if row.devices().iter().any(is_hosted) {
        return direct_hosted_pair(before, after, track).is_some();
    }
    !row.is_group()
        && row.instrument().is_none()
        && row.source_id().is_none()
        && row.sends().is_empty()
        && row.devices().iter().all(|device| {
            device.parent().is_none()
                && device.automation().is_empty()
                && device.modulation().is_empty()
        })
}

fn is_hosted(device: &hex_project::Device) -> bool {
    device.state().type_id().as_str() == Ok("hex.vst3")
}

fn direct_hosted_pair(
    before: &Project,
    after: &Project,
    track: TrackId,
) -> Option<hex_project::DeviceId> {
    // The Engine must still prove the installed runtime routes and clocks.
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
    let hosted = match (is_hosted(first), is_hosted(second)) {
        (true, false) => first,
        (false, true) => second,
        _ => return None,
    };
    Some(hosted.id())
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
