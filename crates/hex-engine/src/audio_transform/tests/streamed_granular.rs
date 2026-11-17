use super::*;

fn setup(mode: WarpMode, looping: bool, neutral_fade: bool) -> (Editor, hex_project::ClipId) {
    let (mut editor, clip, _) = fixture(looping);
    if neutral_fade {
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
    }
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 40_000,
                },
                SourceGainPoint {
                    frame: 4_000,
                    gain_q16: 60_000,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        mode,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 300 })
        .unwrap();
    (editor, clip)
}
fn raw(editor: &Editor, clip: hex_project::ClipId, frames: u64) -> crate::StreamedSourceMetadata {
    crate::StreamedSourceMetadata::new(
        crate::StreamSourceKey::new(1, editor.project().clip(clip).unwrap().asset()),
        16_000,
        16_000,
        frames,
        frames,
        hex_project::IDENTITY_CONVERSION,
        [3; 32],
    )
    .unwrap()
}

#[test]
fn canonical_roots_project_resident_map_region_and_identity() {
    for mode in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 45,
            seed: 9,
        },
    ] {
        for looping in [false, true] {
            let (editor, clip) = setup(mode, looping, true);
            let edit = editor.project().clip(clip).unwrap().audio_edit().unwrap();
            let tempo = editor.project().tempo_map();
            let source = raw(&editor, clip, 8_000);
            let cancel = AtomicBool::new(false);
            let resident = mapping::prepare_map(edit, tempo, &cancel).unwrap().0;
            let projected = streamed_granular_root(&source, edit, tempo, &cancel).unwrap();
            let expected_metadata = projected.metadata().clone();
            let backing = projected.backing_bytes().unwrap();
            let (metadata, region, map, projected_mode, pitch, law) = projected.into_parts();
            assert_eq!(
                backing,
                region.backing_bytes().unwrap() + map.backing_bytes().unwrap()
            );
            assert_eq!(map, resident);
            assert_eq!(projected_mode, render_mode(&edit.domain).unwrap());
            assert_eq!(pitch, edit.domain.pitch_cents);
            assert_eq!(law, render_resampler(&edit.domain));
            assert_eq!(
                (
                    region.source_frames(),
                    region.start(),
                    region.length(),
                    region.phase(),
                    region.looping()
                ),
                (
                    8_000,
                    edit.domain.source_offset,
                    edit.domain.source_length,
                    0,
                    looping
                )
            );
            assert_eq!(
                region.envelope(),
                &[
                    hex_warp::GainPoint {
                        source_frame: 0,
                        gain_q16: 40_000
                    },
                    hex_warp::GainPoint {
                        source_frame: 4_000,
                        gain_q16: 60_000
                    }
                ]
            );
            assert_eq!(metadata, expected_metadata);
            assert_eq!(
                metadata,
                source
                    .with_granular(&region, &map, projected_mode, pitch, law)
                    .unwrap()
            );
            assert_eq!(region.resolve(-1).unwrap().is_some(), looping);
            if looping {
                assert_ne!(edit.domain.source_phase, 0);
            }
        }
    }
}

#[test]
fn granular_projection_refuses_unhandled_inputs_and_projects_long_root() {
    let (editor, clip) = setup(WarpMode::Tones { grain_ms: 32 }, false, true);
    let edit = editor.project().clip(clip).unwrap().audio_edit().unwrap();
    let tempo = editor.project().tempo_map();
    let source = raw(&editor, clip, 8_000);
    let cancel = AtomicBool::new(false);
    assert!(streamed_granular_root(&source, edit, tempo, &AtomicBool::new(true)).is_err());
    let mut invalid = edit.clone();
    invalid.repeat_period = Some(1);
    assert!(streamed_granular_root(&source, &invalid, tempo, &cancel).is_err());
    invalid = edit.clone();
    invalid.content_view = Some(hex_project::AudioContentView {
        restore_plain: false,
        base_view_start: 0,
        base_view_end: 1,
        base_placement: None,
        base_start_delta: 0,
        window: hex_project::ContentWindow::new(0, 1, true).unwrap(),
        read_resampler: hex_project::WarpResampler::V1,
    });
    assert!(streamed_granular_root(&source, &invalid, tempo, &cancel).is_err());
    invalid = edit.clone();
    invalid.placement = Some(hex_project::AudioViewPlacement {
        mode: hex_project::AudioViewMode::Repeat,
        outer_length: invalid.domain.clock,
        cycle_phase: 0,
    });
    assert!(streamed_granular_root(&source, &invalid, tempo, &cancel).is_err());
    invalid = edit.clone();
    Arc::make_mut(&mut invalid.domain).events = Some(Arc::new(hex_project::AudioEvents {
        sources: vec![],
        next_id: 1,
        events: vec![],
    }));
    assert!(streamed_granular_root(&source, &invalid, tempo, &cancel).is_err());
    invalid = edit.clone();
    Arc::make_mut(&mut invalid.domain).musical_event_source =
        Some(hex_project::MusicalEventSource {
            zero_in_destination: 0,
            length: BeatTime::ONE,
        });
    assert!(streamed_granular_root(&source, &invalid, tempo, &cancel).is_err());
    invalid = edit.clone();
    Arc::make_mut(&mut invalid.domain).source_gain = vec![
        SourceGainPoint {
            frame: 0,
            gain_q16: 65_536
        };
        hex_warp::MAX_POINTS + 1
    ];
    assert!(streamed_granular_root(&source, &invalid, tempo, &cancel).is_err());
    invalid = edit.clone();
    Arc::make_mut(&mut invalid.domain).source_length = 8_001;
    assert!(streamed_granular_root(&source, &invalid, tempo, &cancel).is_err());
    invalid = edit.clone();
    Arc::make_mut(&mut invalid.domain)
        .warp
        .as_mut()
        .unwrap()
        .mode = WarpMode::RePitch;
    assert!(streamed_granular_root(&source, &invalid, tempo, &cancel).is_err());
    let (faded, faded_clip) = setup(WarpMode::Tones { grain_ms: 32 }, false, false);
    let faded_edit = faded
        .project()
        .clip(faded_clip)
        .unwrap()
        .audio_edit()
        .unwrap();
    assert!(
        streamed_granular_root(
            &raw(&faded, faded_clip, 8_000),
            faded_edit,
            faded.project().tempo_map(),
            &cancel
        )
        .is_err()
    );
    let mut trimmed = edit.clone();
    let domain = Arc::make_mut(&mut trimmed.domain);
    domain.source_offset = 100;
    domain.source_length = 7_900;
    domain
        .warp
        .as_mut()
        .unwrap()
        .markers
        .first_mut()
        .unwrap()
        .source_frame = 100;
    domain.warp_resampler = hex_project::WarpResampler::V1;
    let projected = streamed_granular_root(&source, &trimmed, tempo, &cancel).unwrap();
    let (_, region, map, _, pitch, law) = projected.into_parts();
    assert_eq!(
        (region.start(), region.length(), pitch, law),
        (100, 7_900, 300, hex_warp::Resampler::V1)
    );
    assert_eq!(
        map,
        mapping::prepare_map(&trimmed, tempo, &cancel).unwrap().0
    );
    let projected = streamed_granular_root(&source, edit, tempo, &cancel).unwrap();
    let (derived, _, _, _, _, _) = projected.into_parts();
    assert!(streamed_granular_root(&derived, edit, tempo, &cancel).is_err());
    let wrong_rate = crate::StreamedSourceMetadata::new(
        source.key(),
        48_000,
        48_000,
        8_000,
        8_000,
        hex_project::IDENTITY_CONVERSION,
        [3; 32],
    )
    .unwrap();
    assert!(streamed_granular_root(&wrong_rate, edit, tempo, &cancel).is_err());
    let mut long = edit.clone();
    let beats = BeatTime::from_raw(BeatTime::ONE.raw() * 2_500);
    let domain = Arc::make_mut(&mut long.domain);
    domain.source_length = 20_000_000;
    domain.clock = AudioDomainClock::Beats(beats);
    let last = domain.warp.as_mut().unwrap().markers.last_mut().unwrap();
    last.source_frame = 20_000_000;
    last.beat = beats;
    long.view_end = beats.raw();
    assert!(mapping::prepare_map(&long, tempo, &cancel).is_err());
    let projected =
        streamed_granular_root(&raw(&editor, clip, 20_000_000), &long, tempo, &cancel).unwrap();
    let (metadata, region, map, _, _, _) = projected.into_parts();
    assert_eq!(metadata.playback_frames(), 20_000_000);
    assert_eq!(region.length(), 20_000_000);
    assert_eq!(map.segments().last().unwrap().output_end, 20_000_000);
}
