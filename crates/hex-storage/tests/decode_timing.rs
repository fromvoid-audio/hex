//! Release timing harness for opening a large current-schema project (Pulse
//! #4015). Ignored by default; run with
//! `cargo test --release -p hex-storage --test decode_timing -- --ignored --nocapture`.
use hex_project::{BeatTime, Command, Created, Editor, Project};
use hex_storage::{decode, encode};
use std::time::Instant;

const TRACKS: usize = 64;
const CLIPS_PER_TRACK: u32 = 16;
const NOTES_PER_CLIP: u32 = 64;

fn large_project() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    for index in 0..TRACKS {
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: format!("track {index}"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        for slot in 0..CLIPS_PER_TRACK {
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::from_quarters(u64::from(slot * 16)).unwrap(),
                    length: BeatTime::from_quarters(16).unwrap(),
                })
                .unwrap()
            else {
                panic!("clip")
            };
            for note in 0..NOTES_PER_CLIP {
                editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_ratio(u64::from(note), 4).unwrap(),
                        duration: BeatTime::from_ratio(1, 4).unwrap(),
                        pitch: 36 + (note % 48) as u8,
                        channel: 0,
                        velocity: 100,
                    })
                    .unwrap();
            }
        }
    }
    editor.project().clone()
}

#[test]
#[ignore = "release timing harness"]
fn large_current_project_decode_timing() {
    let project = large_project();
    let bytes = encode(&project).unwrap();
    assert_eq!(decode(&bytes).unwrap(), project);
    let runs = 10;
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let decoded = decode(&bytes).unwrap();
        samples.push(start.elapsed());
        std::hint::black_box(decoded);
    }
    samples.sort_unstable();
    println!(
        "decode {} bytes ({TRACKS} tracks, {} notes): min {:?} median {:?} max {:?} over {runs} runs",
        bytes.len(),
        TRACKS * (CLIPS_PER_TRACK * NOTES_PER_CLIP) as usize,
        samples[0],
        samples[runs / 2],
        samples[runs - 1],
    );
}
