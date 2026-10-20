use super::*;
use hex_project::{
    BeatTime, Command, Created, Editor, Project, Tempo, TempoMap, TempoPoint, WarpMarker,
    WarpSettings,
};

fn edit() -> AudioClipEdit {
    let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "map".into(),
            path: "no-io.wav".into(),
            sample_rate: 16000,
            frames: 8000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let mut edit = editor.project().clip(clip).unwrap().audio_event_root();
    let domain = Arc::make_mut(&mut edit.domain);
    domain.clock = AudioDomainClock::Beats(BeatTime::ONE);
    domain.warp_enabled = true;
    domain.warp = Some(WarpSettings {
        mode: WarpMode::RePitch,
        markers: vec![
            WarpMarker {
                source_frame: 0,
                beat: BeatTime::ZERO,
            },
            WarpMarker {
                source_frame: 8000,
                beat: BeatTime::ONE,
            },
        ],
    });
    edit.view_end = BeatTime::ONE.raw();
    edit.musical_start = Some(BeatTime::ZERO);
    edit
}
fn validate(
    edit: &AudioClipEdit,
    tempo: &TempoMap,
    scratch: &mut WarpPreviewWorkspace,
) -> Result<(), Invalid> {
    edit.with_warp_map_read(tempo, |read| {
        let frames = prepare_projection(&read, scratch, &AtomicBool::new(false), false)?;
        hex_warp::TimeMapRead::new(&scratch.segments, frames)
            .map_err(warp_error)?
            .validate_stretch(render_mode(&edit.domain)?, edit.domain.pitch_cents)
            .map_err(warp_error)
    })
}
fn same(
    edit: &AudioClipEdit,
    tempo: &TempoMap,
    scratch: &mut WarpPreviewWorkspace,
) -> Result<(), Invalid> {
    let expected = prepare_map(edit, tempo, &AtomicBool::new(false)).and_then(|(map, _)| {
        map.validate_stretch(render_mode(&edit.domain)?, edit.domain.pitch_cents)
            .map_err(warp_error)
    });
    let actual = validate(edit, tempo, scratch);
    assert_eq!(actual, expected);
    actual
}
#[test]
fn workspace_preserves_pre_dedup_bound_cycles_attacks_and_error_order() {
    let tempo = TempoMap::constant(16_000, Tempo::new(1_000_000_000).unwrap()).unwrap();
    let mut scratch = WarpPreviewWorkspace::reserve_on_control().unwrap();
    let mut e = edit();
    let domain = Arc::make_mut(&mut e.domain);
    domain.looping = true;
    domain.clock = AudioDomainClock::Beats(BeatTime::from_raw(4096 * BeatTime::ONE.raw()));
    e.view_end = 4096 * BeatTime::ONE.raw();
    assert_eq!(
        same(&e, &tempo, &mut scratch),
        Err(Invalid(
            "warp stretch including pitch compensation must be within 1/8..8"
        ))
    );
    assert_eq!(scratch.boundaries.len(), 4097);
    // This duplicate tempo/marker boundary exceeds the real raw limit even
    // though sort/dedup would otherwise shrink it back to 4097.
    let duplicate = TempoMap::new(
        16_000,
        vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(1_000_000_000).unwrap()),
            TempoPoint::new(BeatTime::ONE, Tempo::new(1_000_000_000).unwrap()),
        ],
    )
    .unwrap();
    assert_eq!(
        same(&e, &duplicate, &mut scratch),
        Err(Invalid("warp projection exceeds 8192 bounded segments"))
    );
    Arc::make_mut(&mut e.domain).clock =
        AudioDomainClock::Beats(BeatTime::from_raw(8192 * BeatTime::ONE.raw()));
    assert_eq!(
        same(&e, &tempo, &mut scratch),
        Err(Invalid("warp loop density exceeds bounded projection"))
    );
    let tempo = TempoMap::constant(16_000, Tempo::DEFAULT).unwrap();
    let mut e = edit();
    let division = BeatTime::ONE.raw() / 1024;
    for count in [1023_u64, 1024] {
        let unit = count * division;
        let d = Arc::make_mut(&mut e.domain);
        d.clock = AudioDomainClock::Beats(BeatTime::from_raw(unit));
        let w = d.warp.as_mut().unwrap();
        w.markers[1].beat = BeatTime::from_raw(unit);
        w.mode = WarpMode::Beats {
            preserve: BeatsPreserve::Division(BeatTime::from_raw(division)),
            tail: BeatsTail::Forward,
            decay: 100,
        };
        e.view_end = unit;
        let result = same(&e, &tempo, &mut scratch);
        if count == 1023 {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result,
                Err(Invalid("beat division exceeds 1024 source attacks"))
            );
        }
    }
    // Almost all computed source attacks would collapse to frame zero. The
    // original count guard must still refuse BEFORE discarding them.
    let d = Arc::make_mut(&mut e.domain);
    d.source_length = 1;
    d.warp.as_mut().unwrap().markers[1].source_frame = 1;
    assert_eq!(
        same(&e, &tempo, &mut scratch),
        Err(Invalid("beat division exceeds 1024 source attacks"))
    );
}
#[test]
fn workspace_output_extent_and_compensated_ratio_bounds_are_exact() {
    let tempo = TempoMap::constant(16_000, Tempo::DEFAULT).unwrap();
    let mut scratch = WarpPreviewWorkspace::reserve_on_control().unwrap();
    let mut e = edit();
    for length in [16_777_216, 16_777_217] {
        let d = Arc::make_mut(&mut e.domain);
        d.clock = AudioDomainClock::Frames(length);
        d.warp_enabled = false;
        e.musical_start = None;
        e.view_end = length;
        assert_eq!(same(&e, &tempo, &mut scratch).is_ok(), length == 16_777_216);
    }
    let segment = |output| hex_warp::MapSegment {
        output_start: 0,
        output_end: output,
        source_start_q32: 0,
        source_end_q32: 8000_i128 * Q32,
    };
    for (output, pitch, ok) in [
        (1000, 0, true),
        (999, 0, false),
        (64000, 0, true),
        (64001, 0, false),
        (32000, 1200, true),
        (32001, 1200, false),
    ] {
        let segments = [segment(output)];
        let map = hex_warp::TimeMapRead::new(&segments, output as usize).unwrap();
        assert_eq!(
            map.validate_stretch(hex_warp::Mode::Complex, pitch).is_ok(),
            ok
        );
    }
    for count in [8192, 8193] {
        let segments: Vec<_> = (0..count)
            .map(|index| hex_warp::MapSegment {
                output_start: index,
                output_end: index + 1,
                source_start_q32: i128::from(index) * Q32,
                source_end_q32: i128::from(index + 1) * Q32,
            })
            .collect();
        assert_eq!(
            hex_warp::TimeMapRead::new(&segments, count as usize).is_ok(),
            count == 8192
        );
    }
}
