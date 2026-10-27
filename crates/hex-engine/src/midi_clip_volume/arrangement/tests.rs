use super::*;
use crate::PreparedTempoMap;
use hex_project::{Command, Created, Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
fn b(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
#[test]
fn finite_rows_lookup_by_clip_and_hold_exact_incoming_endpoint_through_tail() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let mut clips = Vec::new();
    // Canonical position order differs from stable identity order.
    for start in [4, 1, 7] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: b(start),
                length: b(2),
            })
            .unwrap()
        else {
            panic!("clip");
        };
        clips.push(clip);
    }
    let timing = PreparedTempoMap::prepare(editor.project().tempo_map().clone(), 8000).unwrap();
    assert!(
        PreparedArrangementVolumes::prepare(
            editor.project(),
            editor.project().track(track).unwrap(),
            &timing
        )
        .unwrap()
        .is_none()
    );
    for clip in &clips[..2] {
        editor
            .execute(Command::ReplaceMidiClipVolume {
                clip: *clip,
                volume: MidiClipVolume::new(
                    vec![
                        MidiClipVolumePoint::linear(b(0), MidiClipGain::new(0.0).unwrap()),
                        MidiClipVolumePoint {
                            beat: b(2),
                            incoming: MidiClipGain::new(0.5).unwrap(),
                            value: MidiClipGain::new(2.0).unwrap(),
                        },
                    ],
                    b(2),
                )
                .unwrap(),
            })
            .unwrap();
    }
    let prepared = PreparedArrangementVolumes::prepare(
        editor.project(),
        editor.project().track(track).unwrap(),
        &timing,
    )
    .unwrap()
    .unwrap();
    assert_eq!(prepared.rows().len(), 2);
    assert!(
        prepared
            .rows()
            .windows(2)
            .all(|pair| pair[0].clip < pair[1].clip)
    );
    assert!(
        prepared.clip(clips[2]).is_none(),
        "empty unity needs no owner"
    );
    assert_eq!(
        prepared.allocated_row_bytes().unwrap(),
        prepared.rows().capacity() * std::mem::size_of::<PreparedArrangementClipVolume>()
    );
    assert_eq!(
        prepared.retained_control_metadata_bytes(),
        prepared
            .rows()
            .iter()
            .map(|row| row.program.retained_control_metadata_bytes())
            .sum::<usize>()
    );
    let row = prepared.clip(clips[1]).unwrap();
    assert_eq!(row.start, b(1));
    assert_eq!(
        row.endpoint,
        Some(VolumeEndpoint {
            frame: 12000,
            gain: 0.5
        })
    );
    let ramp = row.segment_or_hold(&timing, 4000, 1).unwrap();
    assert_eq!(
        ramp,
        VolumeRamp {
            start_frame: 4000,
            end_frame: 12000,
            start_gain: 0.0,
            end_gain: 0.5,
            next_boundary: Some(12000)
        }
    );
    assert_eq!(row.segment_or_hold(&timing, 8000, 127).unwrap(), ramp);
    for frame in [12000, 12017, 1_000_000] {
        let held = row.segment_or_hold(&timing, frame, 256).unwrap();
        assert_eq!(
            held,
            VolumeRamp {
                start_frame: 12000,
                end_frame: u64::MAX,
                start_gain: 0.5,
                end_gain: 0.5,
                next_boundary: None
            }
        );
        assert_eq!(
            held.gain_at(frame).unwrap(),
            0.5,
            "terminal right value2 is unplayed"
        );
    }
    assert!(row.segment_or_hold(&timing, 3999, 1).is_err());
    assert!(row.segment_or_hold(&timing, 12000, 0).is_err());
    assert!(row.segment_or_hold(&timing, u64::MAX, 1).is_err());
    assert!(check_budget(hex_project::MAX_CLIPS + 1, 0).is_err());
    assert!(check_budget(1, usize::MAX).is_err());
    let mut other = Editor::new(Project::new(8000).unwrap(), 2).unwrap();
    let Created::Track(other_track) = other
        .execute(Command::AddTrack {
            name: "other".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    assert!(
        PreparedArrangementVolumes::prepare(
            editor.project(),
            other.project().track(other_track).unwrap(),
            &timing
        )
        .is_err()
    );
}
