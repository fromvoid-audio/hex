//! Frozen schema39 bytes written by unmodified ae200f3c before schema40 work.
//! The adjacent provenance records writer/rlib and exact input/output hashes.
use hex_project::{BeatTime, SessionCellContent, SessionCellContentSnapshot, TrackContentSnapshot};
#[test]
fn schema39_migration_defaults_activation_windows_and_preserves_complete_state() {
    for bytes in [
        include_bytes!("fixtures/clip-activation/schema39-midi-ae200f3c.json").as_slice(),
        include_bytes!("fixtures/clip-activation/schema39-audio-ae200f3c.json").as_slice(),
        include_bytes!("fixtures/clip-activation/schema39-session-audio-ae200f3c.json").as_slice(),
    ] {
        let original = bytes.to_vec();
        let project = hex_storage::decode(bytes).unwrap();
        for track in project.tracks() {
            for clip in track.clips() {
                assert!(clip.active());
            }
            for clip in track.midi_clips() {
                assert!(clip.active());
                assert_eq!(clip.playback_offset(), BeatTime::ZERO);
            }
        }
        for cell in project.session_cells() {
            match cell.content() {
                SessionCellContent::Audio(clip) => assert!(clip.active()),
                SessionCellContent::Midi(clip) => {
                    assert!(clip.active());
                    assert_eq!(clip.playback_offset(), BeatTime::ZERO);
                }
                SessionCellContent::Stop => (),
            }
        }
        let current = hex_storage::encode(&project).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&current).unwrap();
        assert_eq!(document["version"], hex_storage::CURRENT_SCHEMA_VERSION);
        assert_eq!(
            hex_storage::decode(&current).unwrap().snapshot(),
            project.snapshot()
        );
        // Reactivation and elapsed windows coexist with every older saved field.
        let mut edited = project.snapshot();
        for track in &mut edited.tracks {
            match &mut track.content {
                TrackContentSnapshot::Hybrid {
                    audio_clips,
                    midi_clips,
                } => {
                    for c in audio_clips {
                        c.active = false;
                    }
                    for c in midi_clips {
                        c.active = false;
                        c.playback_offset_raw = BeatTime::from_quarters(16).unwrap().raw();
                    }
                }
                TrackContentSnapshot::Audio(clips) => {
                    for c in clips {
                        c.active = false;
                    }
                }
                TrackContentSnapshot::Instrument(clips) => {
                    for c in clips {
                        c.active = false;
                        c.playback_offset_raw = BeatTime::from_quarters(16).unwrap().raw();
                    }
                }
            }
        }
        for cell in &mut edited.session.cells {
            match &mut cell.content {
                SessionCellContentSnapshot::Audio(c) => c.active = false,
                SessionCellContentSnapshot::Midi(c) => {
                    c.active = false;
                    c.playback_offset_raw = BeatTime::from_quarters(16).unwrap().raw();
                }
                SessionCellContentSnapshot::Stop => (),
            }
        }
        let edited = hex_project::Project::from_snapshot(edited).unwrap();
        assert_eq!(
            hex_storage::decode(&hex_storage::encode(&edited).unwrap())
                .unwrap()
                .snapshot(),
            edited.snapshot()
        );
        assert_eq!(bytes, original);
    }
}
