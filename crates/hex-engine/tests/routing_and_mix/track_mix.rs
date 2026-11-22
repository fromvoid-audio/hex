use hex_engine::{Audio, PreparedMasterGain, Project as PlaybackProject, Renderer, compile};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, ops::Range};

const RATE: u32 = 48_000;
const RENDER_END: usize = 16_000;

fn fixture(mute_audio: bool, solo_instrument: bool) -> PlaybackProject {
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 32).expect("editor");
    let asset = match editor
        .execute(Command::AddAsset {
            path: "mix-fixture.wav".into(),
            sample_rate: RATE,
            frames: 2_048,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        _ => panic!("asset result"),
    };
    let audio_track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("audio track")
    {
        Created::Track(id) => id,
        _ => panic!("track result"),
    };
    let first = match editor
        .execute(Command::PlaceClip {
            track: audio_track,
            asset,
            start: 0,
            duration: 12_000,
            source_offset: 0,
            source_length: 1_024,
            looping: true,
        })
        .expect("place first clip")
    {
        Created::Clip(id) => id,
        _ => panic!("clip result"),
    };
    editor
        .execute(Command::SetClipFades {
            clip: first,
            fade_in: 1_000,
            fade_out: 1_500,
        })
        .expect("first fades");
    editor
        .execute(Command::SplitClip {
            clip: first,
            frame: 333,
        })
        .expect("split looped fade");
    let second = match editor
        .execute(Command::PlaceClip {
            track: audio_track,
            asset,
            start: 100,
            duration: 12_000,
            source_offset: 64,
            source_length: 512,
            looping: true,
        })
        .expect("place overlapping clip")
    {
        Created::Clip(id) => id,
        _ => panic!("clip result"),
    };
    editor
        .execute(Command::SetClipFades {
            clip: second,
            fade_in: 700,
            fade_out: 1_200,
        })
        .expect("second fades");

    let instrument = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "instrument".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("instrument track")
    {
        Created::Track(id) => id,
        _ => panic!("track result"),
    };
    let midi_clip = match editor
        .execute(Command::PlaceMidiClip {
            track: instrument,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(1).expect("arrangement length"),
        })
        .expect("place MIDI")
    {
        Created::Clip(id) => id,
        _ => panic!("clip result"),
    };
    editor
        .execute(Command::AddMidiNotes {
            clip: midi_clip,
            notes: vec![
                hex_project::MidiNoteInsert {
                    active: true,
                    start: BeatTime::from_ratio(1, 16).expect("note start"),
                    duration: BeatTime::from_ratio(1, 2).expect("note gate"),
                    pitch: 60,
                    channel: 0,
                    velocity: 111,
                },
                hex_project::MidiNoteInsert {
                    active: true,
                    start: BeatTime::from_ratio(1, 8).expect("note start"),
                    duration: BeatTime::from_ratio(1, 4).expect("note gate"),
                    pitch: 67,
                    channel: 0,
                    velocity: 93,
                },
            ],
        })
        .expect("add MIDI notes");

    editor
        .execute(Command::Gain {
            track: audio_track,
            gain: 0.73,
        })
        .expect("audio gain");
    editor
        .execute(Command::Balance {
            track: audio_track,
            balance: 0.27,
        })
        .expect("audio balance");
    editor
        .execute(Command::Mute {
            track: audio_track,
            muted: mute_audio,
        })
        .expect("audio mute");
    editor
        .execute(Command::Gain {
            track: instrument,
            gain: 0.84,
        })
        .expect("instrument gain");
    editor
        .execute(Command::Balance {
            track: instrument,
            balance: -0.31,
        })
        .expect("instrument balance");
    if solo_instrument {
        editor
            .execute(Command::ToggleSolo {
                track: instrument,
                exclusive: true,
            })
            .expect("solo instrument");
    }
    editor
        .execute(Command::MasterGain { gain: 0.62 })
        .expect("master gain");

    let source = (0..2_048)
        .map(|index| {
            let left = (index % 31) as f32 * 0.007 - 0.09;
            let right = (index % 23) as f32 * 0.005 - 0.04;
            [left, right]
        })
        .collect();
    let media = BTreeMap::from([(asset, Audio::new(RATE, source).unwrap())]);
    compile::project(editor.project(), &media).expect("compile mixed project")
}

fn track_mix(project: &PlaybackProject, range: Range<usize>) -> Vec<[f32; 2]> {
    let mut mix = vec![[0.0; 2]; range.len()];
    for track in project.tracks() {
        track
            .render_additive(range.start as u64, &mut mix)
            .expect("render one track source");
    }
    project.master_mix().apply(&mut mix);
    mix
}

fn selected_track_mix(
    project: &PlaybackProject,
    track_index: usize,
    range: Range<usize>,
) -> Vec<[f32; 2]> {
    let mut mix = vec![[0.0; 2]; range.len()];
    project
        .tracks()
        .get(track_index)
        .expect("selected track")
        .render_additive(range.start as u64, &mut mix)
        .expect("render selected source only");
    project.master_mix().apply(&mut mix);
    mix
}

fn partitioned_source_mix(project: &PlaybackProject, block_sizes: &[usize]) -> Vec<[f32; 2]> {
    let mut result = Vec::with_capacity(RENDER_END);
    let mut position = 0;
    let mut block_index = 0;
    while position < RENDER_END {
        let block_size = block_sizes[block_index % block_sizes.len()];
        let end = (position + block_size).min(RENDER_END);
        result.extend(track_mix(project, position..end));
        position = end;
        block_index += 1;
    }
    result
}

fn partitioned_renderer(project: &PlaybackProject, block_sizes: &[usize]) -> Vec<[f32; 2]> {
    let mut renderer = Renderer::new(project);
    renderer.play();
    let mut result = Vec::with_capacity(RENDER_END);
    let mut position = 0;
    let mut block_index = 0;
    while position < RENDER_END {
        let block_size = block_sizes[block_index % block_sizes.len()];
        let end = (position + block_size).min(RENDER_END);
        let mut output = vec![[0.0; 2]; end - position];
        renderer.render(&mut output).expect("render arrangement");
        result.extend(output);
        position = end;
        block_index += 1;
    }
    result
}

#[test]
fn borrowed_track_additive_mix_matches_finite_renderer_exactly_across_partitions_and_controls() {
    let irregular = [1, 73, 256, 19, 1_003, 7, 4_096, 31];
    for project in [
        fixture(false, false),
        fixture(true, false),
        fixture(false, true),
    ] {
        let reference = partitioned_renderer(&project, &[RENDER_END]);
        assert_eq!(partitioned_source_mix(&project, &[RENDER_END]), reference);
        assert_eq!(partitioned_source_mix(&project, &irregular), reference);
        assert_eq!(partitioned_renderer(&project, &irregular), reference);
    }
}

#[test]
fn track_and_mix_helpers_refuse_overflow_without_touching_output() {
    let project = fixture(false, false);
    let track = project.tracks().first().expect("audio track");
    let mut unchanged = [[0.125, -0.25]; 2];
    let before = unchanged;
    assert!(track.render_additive(u64::MAX, &mut unchanged).is_err());
    assert_eq!(unchanged, before);

    let mix = hex_engine::PreparedTrackMix::new(0.5, 0.25, false).expect("mix");
    assert_eq!(mix.channel_gain(), [0.375, 0.5]);
    let mut frame = [0.25, -0.25];
    mix.add_frame(&mut frame, [1.0, 2.0]);
    assert_eq!(frame, [0.625, 0.75]);
    let muted = hex_engine::PreparedTrackMix::new(1.0, 0.0, true).expect("muted mix");
    muted.add_frame(&mut frame, [9.0, 9.0]);
    assert_eq!(frame, [0.625, 0.75]);

    let master = PreparedMasterGain::new(0.5).expect("master");
    assert_eq!(master.value(), 0.5);
    let mut frames = [frame];
    master.apply(&mut frames);
    assert_eq!(frames, [[0.3125, 0.375]]);
}

#[test]
fn selecting_one_borrowed_track_does_not_render_or_subtract_other_arrangement_sources() {
    let project = fixture(false, false);
    let range = 2_700..4_200;
    let audio_only = selected_track_mix(&project, 0, range.clone());
    let instrument_only = selected_track_mix(&project, 1, range.clone());
    let full = track_mix(&project, range);

    assert!(audio_only.iter().flatten().any(|sample| *sample != 0.0));
    assert!(
        instrument_only
            .iter()
            .flatten()
            .any(|sample| *sample != 0.0)
    );
    assert_ne!(instrument_only, full);
    assert_ne!(audio_only, full);
    assert_ne!(audio_only, instrument_only);

    // The excluded Arrangement source has real signal in this region; selecting
    // the instrument above did not derive its result by subtracting a full mix.
    let mut excluded_audio = vec![[0.0; 2]; 1_500];
    project.tracks()[0]
        .render_additive(2_700, &mut excluded_audio)
        .expect("render excluded source for the assertion");
    assert!(excluded_audio.iter().flatten().any(|sample| *sample != 0.0));
}
