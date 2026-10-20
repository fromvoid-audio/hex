//! Control-side model reads only, not callback, PCM, OS audio or native UI.
//! Fixtures, canonical command oracles and reclamation remain outside measure.
#![cfg(feature = "allocation-probe")]
use allocation_counter::measure;
use hex_project::{
    AudioClipPreviewOperation as Op, AudioDomainClock, AudioEvent, AudioEvents, AudioViewMode,
    AudioViewPlacement, BeatTime, ClipId, Command, Created, Editor, Project, SourceGainPoint,
    Tempo, TempoPoint, TrimEdge, WarpMarker, WarpMode, WarpSettings,
};
use std::{hint::black_box, sync::Arc};
const Q: u64 = 1 << 32;
fn fixture(retained: bool, maximum: bool, placement: bool) -> (Project, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "allocation".into(),
            path: "not-opened.wav".into(),
            sample_rate: 48_000,
            frames: 480_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    if retained {
        let count = if maximum { 1024 } else { 3 };
        let markers = (0..count)
            .map(|i| WarpMarker {
                source_frame: 480_000 * i as u64 / (count - 1) as u64,
                beat: BeatTime::from_raw(20 * Q * i as u64 / (count - 1) as u64),
            })
            .collect();
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(WarpSettings {
                    mode: WarpMode::RePitch,
                    markers,
                }),
            })
            .unwrap();
    }
    let mut snapshot = editor.project().snapshot();
    if maximum {
        snapshot.tempo_points = (0..1024)
            .map(|i| TempoPoint::new(BeatTime::from_raw(Q * i), Tempo::new(120_000_000).unwrap()))
            .collect();
        let edit = snapshot.tracks[0].audio_clips_mut().unwrap()[0]
            .edit
            .as_mut()
            .unwrap();
        let domain = Arc::make_mut(&mut edit.domain);
        domain.source_gain = (0..1024)
            .map(|i| SourceGainPoint {
                frame: 480_000 * i / 1023,
                gain_q16: 65_536,
            })
            .collect();
        domain.events = Some(Arc::new(AudioEvents {
            next_id: 1025,
            events: (0..1024)
                .map(|i| AudioEvent {
                    id: i + 1,
                    start: 20 * Q * i / 1024,
                    end: 20 * Q * (i + 1) / 1024,
                    source_start: 20 * Q * i / 1024,
                })
                .collect(),
        }));
    }
    if placement {
        let value = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
        let edit = value.edit.as_mut().unwrap();
        edit.placement = Some(AudioViewPlacement {
            mode: AudioViewMode::Repeat,
            outer_length: AudioDomainClock::Beats(BeatTime::from_raw(20 * Q)),
            cycle_phase: Q / 2,
        });
        // A selected cycle and nonzero phase make its technical root origin negative.
        edit.view_end = 10 * Q;
    }
    (Project::from_snapshot(snapshot).unwrap(), clip)
}
#[test]
fn scoped_trim_fade_stretch_projection_has_zero_total_and_net_allocations() {
    for (retained, maximum, placement) in [
        (false, false, false),
        (true, false, false),
        (true, true, false),
        (true, true, true),
    ] {
        let (project, id) = fixture(retained, maximum, placement);
        let clip = project.clip(id).unwrap();
        let tempo = project.tempo_map();
        for (operation, frame) in [
            (Op::Trim(TrimEdge::Left), 24_000),
            (Op::FadeTrim(TrimEdge::Right), 456_000),
            (Op::Stretch(TrimEdge::Right), 600_000),
            (Op::Stretch(TrimEdge::Left), 24_000),
        ] {
            let mut result = Ok(());
            let measured = measure(|| {
                result = clip
                    .with_audio_preview(
                        operation,
                        frame,
                        480_000,
                        tempo,
                        |read| -> Result<(), hex_project::Invalid> {
                            let facts = black_box(read.facts());
                            let (_, length, _, _) = read.projected_domain()?;
                            // One construction/validation, then many projected columns.
                            for index in 0..128 {
                                let root = (length - 1) * index / 127;
                                black_box(read.fade_gain_at(root)?);
                                black_box(read.fade_frame_at(root)?);
                                black_box(read.accepted_waveform_position(
                                    facts.start + facts.duration * index / 127,
                                )?);
                            }
                            let count = read.occurrence_count()?;
                            for index in 0..count.min(8) {
                                black_box(read.occurrence(index)?);
                            }
                            Ok(())
                        },
                    )
                    .and_then(|r| r);
            });
            assert_eq!(
                result,
                Ok(()),
                "retained={retained} maximum={maximum} placement={placement} operation={operation:?}"
            );
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0),
                "{operation:?}: {measured:?}"
            );
            eprintln!(
                "audio_preview retained={retained} maximum={maximum} placement={placement} operation={operation:?} allocations={measured:?}"
            );
        }
    }
}

#[test]
fn default_warp_copy_endpoints_have_zero_total_and_net_allocations() {
    use hex_project::{default_audio_warp, default_audio_warp_markers};
    for (retained, maximum, placement) in [
        (false, false, false),
        (true, false, false),
        (true, true, false),
        (true, true, true),
    ] {
        let (project, id) = fixture(retained, maximum, placement);
        let clip = project.clip(id).unwrap();
        let tempo = project.tempo_map();
        let expected = default_audio_warp(clip, tempo, WarpMode::RePitch).unwrap();
        let mut result = Ok([WarpMarker {
            source_frame: 0,
            beat: BeatTime::ZERO,
        }; 2]);
        let measured = measure(|| {
            for _ in 0..64 {
                result = black_box(default_audio_warp_markers(clip, tempo));
            }
        });
        assert_eq!(result.unwrap().as_slice(), expected.markers);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0),
            "{measured:?}"
        );
        eprintln!(
            "default_warp_copy retained={retained} maximum={maximum} placement={placement} calls=64 allocations={measured:?}"
        );
    }
}
