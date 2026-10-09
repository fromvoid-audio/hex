use hex_engine::{Audio, Clip, Frame, Project, Renderer, Track};

#[derive(Clone, Copy)]
struct ClipSpec {
    start: u64,
    duration: u64,
    source_offset: u64,
    source_length: u64,
    looping: bool,
}

struct TrackSpec {
    clips: Vec<ClipSpec>,
    gain: f32,
    muted: bool,
}

fn fixture() -> (Project, Vec<TrackSpec>, Vec<Frame>) {
    let source = (0..7)
        .map(|i| [i as f32 * 0.125 - 0.25, 0.5 - i as f32 * 0.0625])
        .collect::<Vec<_>>();
    let audio = Audio::new(48_000, source.clone()).expect("valid source");
    let tracks = vec![
        TrackSpec {
            clips: vec![
                ClipSpec {
                    start: 13,
                    duration: 12,
                    source_offset: 2,
                    source_length: 3,
                    looping: true,
                },
                ClipSpec {
                    start: 0,
                    duration: 10,
                    source_offset: 0,
                    source_length: 7,
                    looping: false,
                },
                ClipSpec {
                    start: 7,
                    duration: 14,
                    source_offset: 1,
                    source_length: 4,
                    looping: false,
                },
            ],
            gain: 0.75,
            muted: false,
        },
        TrackSpec {
            clips: vec![
                ClipSpec {
                    start: 8,
                    duration: 19,
                    source_offset: 3,
                    source_length: 2,
                    looping: true,
                },
                ClipSpec {
                    start: 4,
                    duration: 5,
                    source_offset: 0,
                    source_length: 7,
                    looping: false,
                },
            ],
            gain: 0.5,
            muted: false,
        },
        TrackSpec {
            clips: vec![ClipSpec {
                start: 0,
                duration: 40,
                source_offset: 0,
                source_length: 7,
                looping: true,
            }],
            gain: 3.0,
            muted: true,
        },
        TrackSpec {
            clips: vec![
                ClipSpec {
                    start: 27,
                    duration: 11,
                    source_offset: 4,
                    source_length: 3,
                    looping: false,
                },
                ClipSpec {
                    start: 16,
                    duration: 8,
                    source_offset: 1,
                    source_length: 5,
                    looping: true,
                },
            ],
            gain: 1.25,
            muted: false,
        },
    ];
    let engine_tracks = tracks
        .iter()
        .map(|track| {
            let clips = track
                .clips
                .iter()
                .map(|clip| {
                    Clip::region(
                        audio.clone(),
                        clip.start,
                        clip.duration,
                        clip.source_offset,
                        clip.source_length,
                        clip.looping,
                    )
                    .expect("valid clip region")
                })
                .collect();
            Track::new(clips, track.gain, track.muted).expect("valid track")
        })
        .collect();
    (
        Project::new(48_000, engine_tracks).expect("valid project"),
        tracks,
        source,
    )
}

/// Deliberately slow scalar oracle: examine every track and clip for every frame.
fn render_scalar(tracks: &[TrackSpec], source: &[Frame], position: u64, output: &mut [Frame]) {
    output.fill([0.0; 2]);
    for (index, frame) in output.iter_mut().enumerate() {
        let timeline = position + index as u64;
        for track in tracks {
            if track.muted {
                continue;
            }
            for clip in &track.clips {
                let end = clip.start + clip.duration;
                if timeline < clip.start || timeline >= end {
                    continue;
                }
                let relative = timeline - clip.start;
                if !clip.looping && relative >= clip.source_length {
                    continue;
                }
                let source_index = clip.source_offset + relative % clip.source_length;
                let sample = source[source_index as usize];
                frame[0] += sample[0] * track.gain;
                frame[1] += sample[1] * track.gain;
            }
        }
    }
}

#[test]
fn clip_spans_match_scalar_oracle_across_partitions_and_seek() {
    let (project, tracks, source) = fixture();
    let mut renderer = Renderer::new(&project);
    renderer.play();
    let mut position = 0_u64;
    for size in [0, 1, 5, 2, 13, 0, 8, 1, 17, 3, 11] {
        let mut expected = vec![[99.0; 2]; size];
        let mut actual = expected.clone();
        render_scalar(&tracks, &source, position, &mut expected);
        renderer.render(&mut actual).expect("render block");
        assert_eq!(actual, expected, "partition size {size} at {position}");
        position += size as u64;
        assert_eq!(renderer.position(), position);
    }

    renderer.seek(6);
    position = 6;
    for size in [7, 1, 16, 4, 0, 9] {
        let mut expected = vec![[0.0; 2]; size];
        let mut actual = expected.clone();
        render_scalar(&tracks, &source, position, &mut expected);
        renderer.render(&mut actual).expect("render after seek");
        assert_eq!(actual, expected, "partition size {size} after seek");
        position += size as u64;
        assert_eq!(renderer.position(), position);
    }
}
