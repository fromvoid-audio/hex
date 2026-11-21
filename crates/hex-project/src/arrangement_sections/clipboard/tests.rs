use super::*;
use crate::{
    AutomationInterpolation, AutomationPoint, AutomationValue, DeviceAutomationTarget, Editor,
    ElementColor, GainInterpolation, GainPoint, Meter, Tempo,
};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState, ParameterId};

use super::super::test_support::{beat, fixture};

fn paste_cmd(clipboard: &Arc<SectionClipboard>, at: u64) -> Command {
    Command::PasteArrangementSection {
        clipboard: Arc::clone(clipboard),
        at: beat(at),
        gap_name: "gap".into(),
        gap_color: ElementColor::Rose,
    }
}

fn geometry(sections: &[ArrangementSection]) -> Vec<(&str, ElementColor, BeatTime, BeatTime)> {
    sections
        .iter()
        .map(|s| (s.name(), s.color(), s.start(), s.end()))
        .collect()
}

#[test]
fn paste_at_boundary_gap_interior_split_and_beyond_end_match_preview_and_undo() {
    for (at, expected_count, splits_second, has_gap) in [
        (0, 3, false, false),
        (4, 3, false, false),
        (6, 4, true, false),
        (8, 3, false, false),
        (12, 4, false, true),
    ] {
        let (mut editor, source) = fixture();
        let second_id = editor.project().arrangement_sections()[1].id();
        let clipboard = Arc::new(
            editor
                .project()
                .capture_arrangement_section(source)
                .unwrap(),
        );
        let before = editor.project().snapshot();
        let cmd = paste_cmd(&clipboard, at);
        assert!(cmd.requires_section_publication());
        let preview = editor
            .project()
            .preview_arrangement_section_edit(&cmd)
            .unwrap();
        assert_eq!(editor.project().snapshot(), before, "preview is read-only");
        let Created::ArrangementSection(pasted) = editor.execute(cmd).unwrap() else {
            panic!("expected pasted section");
        };
        let after = editor.project().snapshot();
        let project = editor.project();
        assert_eq!(project.arrangement_sections().len(), expected_count);
        assert_eq!(
            geometry(project.arrangement_sections()),
            geometry(preview.sections())
        );
        assert_eq!(project.tempo_map(), preview.tempo_map());
        let marker = project.arrangement_section(pasted).unwrap();
        assert_eq!((marker.start(), marker.end()), (beat(at), beat(at + 4)));
        assert_eq!(marker.name(), clipboard.name());
        assert_eq!(marker.color(), clipboard.color());
        if splits_second {
            let left = project.arrangement_section(second_id).unwrap();
            assert_eq!((left.start(), left.end()), (beat(4), beat(6)));
            let right = project
                .arrangement_sections()
                .iter()
                .find(|s| s.start() == beat(10) && s.end() == beat(12))
                .expect("split right half");
            assert_ne!(right.id(), second_id);
            assert_eq!(right.name(), left.name());
            assert_eq!(right.color(), left.color());
        }
        let gap = project
            .arrangement_sections()
            .iter()
            .find(|s| s.name() == "gap");
        assert_eq!(gap.is_some(), has_gap);
        if let Some(gap) = gap {
            assert_eq!((gap.start(), gap.end()), (beat(8), beat(12)));
        }
        // Audio and MIDI content copied into the pasted range.
        assert_eq!(project.tracks()[0].clips().len(), 2);
        assert!(
            project.tracks()[0]
                .clips()
                .iter()
                .any(|c| c.start() == at * 4_000)
        );
        assert!(
            project.tracks()[1]
                .midi_clips()
                .iter()
                .any(|c| c.start() == beat(at) && c.notes()[0].pitch() == 60)
        );
        // Undo / Redo round-trip.
        assert!(editor.undo());
        let mut undone = editor.project().snapshot();
        assert!(undone.next_id >= after.next_id);
        undone.next_id = before.next_id;
        assert_eq!(undone, before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}

#[test]
fn cut_and_paste_preserves_tempo_meter_gain_locators_and_device_automation() {
    let (mut editor, source) = fixture();
    let track = editor.project().tracks()[0].id();
    // Add tempo, meter, gain, locator, and device automation inside [0, 4).
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(2),
            tempo: Tempo::new(140_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetMeterPoint {
            beat: beat(2),
            meter: Meter::new(3, 4).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::UpsertTrackGainPoint {
            track,
            point: GainPoint {
                beat: beat(1),
                gain: 0.5,
                outgoing: GainInterpolation::Linear,
                incoming_gain: None,
            },
        })
        .unwrap();
    let loc_frame = editor.project().tempo_map().beat_to_frame(beat(1)).unwrap();
    editor
        .execute(Command::AddArrangementLocator { frame: loc_frame })
        .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: NativeDeviceState::new(
                DeviceTypeId::new("test.automation").unwrap(),
                1,
                vec![1],
            )
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("expected device");
    };
    let parameter = ParameterId::from_static(1);
    let target = DeviceAutomationTarget::Parameter(parameter);
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter,
            points: vec![
                AutomationPoint {
                    beat: beat(0),
                    value: AutomationValue::new(0.25).unwrap(),
                    outgoing: AutomationInterpolation::Linear,
                },
                AutomationPoint {
                    beat: beat(3),
                    value: AutomationValue::new(0.75).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();

    // Capture, then Cut (DeleteArrangementSection) both sections so the lane is empty.
    let clipboard = Arc::new(
        editor
            .project()
            .capture_arrangement_section(source)
            .unwrap(),
    );
    editor
        .execute(Command::DeleteArrangementSection(source))
        .unwrap();
    let second = editor.project().arrangement_sections()[0].id();
    editor
        .execute(Command::DeleteArrangementSection(second))
        .unwrap();
    assert!(editor.project().arrangement_sections().is_empty());

    // Paste into an empty section lane at beat 4 -> creates gap [0, 4) and pasted [4, 8).
    let Created::ArrangementSection(pasted) = editor.execute(paste_cmd(&clipboard, 4)).unwrap()
    else {
        panic!("expected pasted section");
    };
    let project = editor.project();
    assert_eq!(project.arrangement_sections().len(), 2);
    assert_eq!(
        (
            project.arrangement_sections()[0].name(),
            project.arrangement_sections()[0].start(),
            project.arrangement_sections()[0].end()
        ),
        ("gap", beat(0), beat(4))
    );
    let marker = project.arrangement_section(pasted).unwrap();
    assert_eq!((marker.start(), marker.end()), (beat(4), beat(8)));
    assert_eq!(
        project.tempo_map().tempo_at(beat(6)),
        Tempo::new(140_000_000).unwrap()
    );
    assert_eq!(
        project.meter_map().meter_at(beat(6)),
        Meter::new(3, 4).unwrap()
    );
    assert_eq!(project.arrangement_locators().len(), 1);
    let pasted_device = project.device(device).unwrap();
    let lane = pasted_device
        .automation()
        .iter()
        .find(|lane| lane.target == target)
        .expect("restored automation lane after cut");
    assert_eq!(
        lane.value_at(beat(4)).unwrap().get(),
        0.25,
        "start automation value restored"
    );
    assert_eq!(
        lane.value_at(beat(7)).unwrap().get(),
        0.75,
        "interior automation value restored"
    );
}

#[test]
fn paste_at_zero_in_empty_section_lane_adds_no_gap_and_validates_errors() {
    let (mut editor, source) = fixture();
    let clipboard = Arc::new(
        editor
            .project()
            .capture_arrangement_section(source)
            .unwrap(),
    );
    editor
        .execute(Command::DeleteArrangementSection(source))
        .unwrap();
    let second = editor.project().arrangement_sections()[0].id();
    editor
        .execute(Command::DeleteArrangementSection(second))
        .unwrap();
    let Created::ArrangementSection(pasted) = editor.execute(paste_cmd(&clipboard, 0)).unwrap()
    else {
        panic!("expected pasted section");
    };
    assert_eq!(editor.project().arrangement_sections().len(), 1);
    assert_eq!(
        editor
            .project()
            .arrangement_section(pasted)
            .unwrap()
            .start(),
        beat(0)
    );

    // Sample-rate mismatch refuses cleanly.
    let mut other = Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
    assert!(other.execute(paste_cmd(&clipboard, 0)).is_err());

    // Invalid gap name refuses cleanly.
    assert!(
        editor
            .execute(Command::PasteArrangementSection {
                clipboard: Arc::clone(&clipboard),
                at: beat(8),
                gap_name: "   ".into(),
                gap_color: ElementColor::Rose,
            })
            .is_err()
    );
}
