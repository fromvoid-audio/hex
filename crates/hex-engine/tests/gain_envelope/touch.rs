//! Saved overlay playback exercises the same immutable mix law in every renderer.
use super::*;
use hex_engine::{GainAutomationPreview, OutputCapture, OutputCaptureId};
use hex_project::{GainTouchContext, GainTouchSpan, GainTouchStep};

fn span(start: BeatTime, end: BeatTime, epoch: u64, values: &[(BeatTime, f32)]) -> GainTouchSpan {
    GainTouchSpan {
        source_start: start,
        start,
        end,
        context: GainTouchContext {
            recording_epoch: epoch,
            project_generation: 0,
            canonical_revision: 1,
            audio_revision: 1,
            transport_epoch: 1,
            gesture_epoch: 1,
            gesture_serial: epoch,
            sample_rate: 48000,
            loop_pass: 0,
        },
        steps: values
            .iter()
            .enumerate()
            .map(|(i, &(beat, gain))| GainTouchStep {
                beat,
                gain,
                origin_beat: beat,
                source_sequence: i as u64 + 1,
            })
            .collect(),
    }
}
fn splice(f: &mut Fixture, expected_epoch: u64, span: GainTouchSpan) {
    f.editor
        .execute(Command::SpliceGainTouch {
            track: f.track,
            expected_epoch,
            span,
        })
        .unwrap();
}
fn quarter(numerator: u64, denominator: u64) -> BeatTime {
    BeatTime::from_raw(BeatTime::ONE.raw() * numerator / denominator)
}

fn ramp(from: f32, target: f32, elapsed: usize) -> f32 {
    if elapsed >= 240 {
        return target;
    }
    (f64::from(from) + (f64::from(target) - f64::from(from)) * elapsed as f64 / 240.0) as f32
}

#[test]
fn recorded_touch_ramps_rapid_changes_and_release_consistently_after_seek() {
    let mut f = fixture(1024);
    let at = |frame| f.editor.project().tempo_map().frame_to_beat(frame).unwrap();
    let start = at(64);
    let changed = at(128);
    let end = at(512);
    splice(
        &mut f,
        0,
        span(start, end, 1, &[(start, 0.8), (changed, 0.2)]),
    );
    let value = |frame: usize| {
        if frame < 64 {
            0.0
        } else if frame < 128 {
            0.8 * (frame - 63) as f32 / 240.0
        } else if frame < 512 {
            let from = 0.8 * 64.0 / 240.0;
            from + (0.2 - from) * (frame - 127).min(240) as f32 / 240.0
        } else {
            0.2 * (1.0 - (frame - 511).min(240) as f32 / 240.0)
        }
    };
    let expected: Vec<_> = (0..1024)
        .map(|frame| [0.25 * value(frame), -0.5 * value(frame)])
        .collect();
    let revision = revision(&f);
    let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = vec![[0.0; 2]; 1024];
    for block in actual.chunks_mut(37) {
        runtime.render(block).unwrap();
    }
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        for channel in 0..2 {
            assert!(
                (actual[channel] - expected[channel]).abs() < 1e-6,
                "frame {index}: {actual:?} != {expected:?}"
            );
        }
    }
    let mut seek = SessionPlayback::new(revision).unwrap();
    seek.apply(SessionPlaybackCommand::Seek(200)).unwrap();
    seek.apply(SessionPlaybackCommand::Play).unwrap();
    let mut after_seek = [[0.0; 2]; 64];
    seek.render(&mut after_seek).unwrap();
    assert_eq!(&after_seek, &actual[200..264]);
    let preview = GainAutomationPreview::prepare(f.editor.project()).unwrap();
    for frame in [0, 64, 127, 128, 367, 511, 512, 751, 900] {
        assert!((preview.value_at(f.track, frame as u64).unwrap() - value(frame)).abs() < 1e-6);
    }
    let mut segments = Vec::new();
    assert!(preview.visit_segments(f.track, 0, 1024, |segment| segments.push(segment)));
    for segment in segments {
        for frame in segment.start_frame..segment.end_frame {
            let fraction = (frame - segment.start_frame) as f64
                / (segment.end_frame - segment.start_frame) as f64;
            let displayed = (f64::from(segment.start_gain)
                + f64::from(segment.end_gain - segment.start_gain) * fraction)
                as f32;
            assert!(
                (displayed - value(frame as usize)).abs() < 1e-6,
                "preview frame {frame}"
            );
        }
    }
}

#[test]
fn interrupted_touch_return_lands_on_read_without_anticipating_the_next_span() {
    let mut f = fixture(1024);
    let at = |frame| f.editor.project().tempo_map().frame_to_beat(frame).unwrap();
    let (b64, b80, b96, b128, b200, b256, b320, b400) = (
        at(64),
        at(80),
        at(96),
        at(128),
        at(200),
        at(256),
        at(320),
        at(400),
    );
    lane(
        &mut f,
        vec![
            point(BeatTime::ZERO, 0.1, GainInterpolation::Linear),
            point(b256, 0.4, GainInterpolation::Step),
            point(b320, 0.9, GainInterpolation::Step),
        ],
    );
    splice(
        &mut f,
        0,
        span(b64, b128, 1, &[(b64, 0.8), (b80, 0.8), (b96, 0.8)]),
    );
    splice(&mut f, 1, span(b200, b400, 2, &[(b200, 0.4)]));
    let preview = GainAutomationPreview::prepare(f.editor.project()).unwrap();
    let base_before = (0.1_f64 + (0.4_f64 - 0.1_f64) * 63.0 / 256.0) as f32;
    let first_end = ramp(base_before, 0.8, 64);
    let before_second = ramp(first_end, 0.9, 72);
    let second_end = ramp(before_second, 0.4, 200);
    let expected = |frame: u64| match frame {
        64..=127 => ramp(base_before, 0.8, (frame - 63) as usize),
        128..=199 => ramp(first_end, 0.9, (frame - 127) as usize),
        200..=399 => ramp(before_second, 0.4, (frame - 199) as usize),
        400..=639 => ramp(second_end, 0.9, (frame - 399) as usize),
        _ => 0.9,
    };
    for frame in [
        64, 80, 96, 127, 128, 199, 200, 256, 320, 399, 400, 638, 639, 640,
    ] {
        let actual = preview.value_at(f.track, frame).unwrap();
        assert!(
            (actual - expected(frame)).abs() < 1e-6,
            "frame {frame}: {actual} != {}",
            expected(frame)
        );
    }
    let mut segments = Vec::new();
    assert!(preview.visit_segments(f.track, 64, 641, |segment| segments.push(segment)));
    for segment in segments {
        for frame in segment.start_frame..segment.end_frame {
            let fraction = (frame - segment.start_frame) as f64
                / (segment.end_frame - segment.start_frame) as f64;
            let displayed = (f64::from(segment.start_gain)
                + f64::from(segment.end_gain - segment.start_gain) * fraction)
                as f32;
            assert!(
                (displayed - expected(frame)).abs() < 1e-6,
                "preview frame {frame}"
            );
        }
    }
}

#[test]
fn spliced_touch_fragment_starts_from_the_preceding_audible_gain() {
    let mut f = fixture(1024);
    let at = |frame| f.editor.project().tempo_map().frame_to_beat(frame).unwrap();
    let (start, cut, resume, end) = (at(64), at(128), at(192), at(512));
    splice(&mut f, 0, span(start, end, 1, &[(start, 0.8)]));
    splice(&mut f, 1, span(cut, resume, 2, &[(cut, 0.4)]));
    let preview = GainAutomationPreview::prepare(f.editor.project()).unwrap();
    let first_end = ramp(0.0, 0.8, 64);
    let second_end = ramp(first_end, 0.4, 64);
    for (frame, expected) in [
        (127, first_end),
        (128, ramp(first_end, 0.4, 1)),
        (191, second_end),
        (192, ramp(second_end, 0.8, 1)),
    ] {
        assert!(
            (preview.value_at(f.track, frame).unwrap() - expected).abs() < 1e-6,
            "frame {frame}"
        );
    }
}

#[test]
fn touch_ramp_duration_scales_with_sample_rate() {
    for rate in [44_100, 48_000, 96_000] {
        let mut editor = Editor::new(Project::new(rate).unwrap(), 8).unwrap();
        let Created::Imported { track, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "source".into(),
                path: "memory.wav".into(),
                source_sample_rate: rate,
                source_frames: 2048,
                playback_frames: 2048,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
        let end = editor.project().tempo_map().frame_to_beat(1024).unwrap();
        let mut recorded = span(BeatTime::ZERO, end, 1, &[(BeatTime::ZERO, 0.8)]);
        recorded.context.sample_rate = rate;
        editor
            .execute(Command::SpliceGainTouch {
                track,
                expected_epoch: 0,
                span: recorded,
            })
            .unwrap();
        let preview = GainAutomationPreview::prepare(editor.project()).unwrap();
        let duration = u64::from(rate.div_ceil(200));
        assert!((preview.value_at(track, 0).unwrap() - 0.8 / duration as f32).abs() < 1e-6);
        assert_eq!(preview.value_at(track, duration - 1), Some(0.8));
        assert_eq!(preview.value_at(track, 1024 + duration - 1), Some(0.0));
    }
}

#[test]
fn touch_read_preserves_tempo_split_base_ramps_and_adjacent_half_open_spans() {
    let mut f = fixture(72002);
    f.editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ONE,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let base = vec![
        point(BeatTime::ZERO, 0.0, GainInterpolation::Linear),
        point(quarter(2, 1), 2.0, GainInterpolation::Step),
    ];
    lane(&mut f, base.clone());
    splice(
        &mut f,
        0,
        span(
            quarter(1, 2),
            quarter(3, 2),
            1,
            &[
                (quarter(1, 2), 0.9),
                (quarter(3, 4), 0.4),
                (BeatTime::ONE, 0.7),
            ],
        ),
    );
    splice(
        &mut f,
        1,
        span(quarter(3, 2), quarter(7, 4), 2, &[(quarter(3, 2), 1.8)]),
    );
    assert_eq!(
        f.editor.project().track(f.track).unwrap().gain_envelope(),
        base
    );
    let preview = GainAutomationPreview::prepare(f.editor.project()).unwrap();
    let base_gain = |frame: usize| {
        if frame <= 24000 {
            (frame as f64 / 24000.0) as f32
        } else if frame < 72000 {
            (1.0 + (frame - 24000) as f64 / 48000.0) as f32
        } else {
            2.0
        }
    };
    let expected_gain = |frame: usize| {
        if (12000..18000).contains(&frame) {
            ramp(base_gain(11999), 0.9, frame - 11999)
        } else if (18000..24000).contains(&frame) {
            ramp(0.9, 0.4, frame - 17999)
        } else if (24000..48000).contains(&frame) {
            ramp(0.4, 0.7, frame - 23999)
        } else if (48000..60000).contains(&frame) {
            ramp(0.7, 1.8, frame - 47999)
        } else if (60000..60240).contains(&frame) {
            ramp(1.8, base_gain(60239), frame - 59999)
        } else {
            base_gain(frame)
        }
    };
    let expected: Vec<_> = (0..f.frames)
        .map(|frame| [0.25 * expected_gain(frame), -0.5 * expected_gain(frame)])
        .collect();
    assert_eq!(arrangement(&f), expected);
    let r = revision(&f);
    let capacity = r.arrangement().gain_automation_capacity();
    assert_eq!(capacity.authored_units, 10);
    assert_eq!(
        (capacity.prepared_touch_spans, capacity.prepared_touch_steps),
        (2, 4)
    );
    assert!(capacity.allocated_touch_bytes > 0);
    assert!(
        capacity.allocated_touch_bytes + capacity.allocated_knot_bytes
            <= hex_engine::MAX_PREPARED_GAIN_BYTES
    );
    for linear in [false, true] {
        let mut runtime = if linear {
            SessionPlayback::new_linear_arrangement_export(Arc::clone(&r))
        } else {
            SessionPlayback::new(Arc::clone(&r))
        }
        .unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut actual = vec![[0.0; 2]; f.frames];
        for block in actual.chunks_mut(257) {
            runtime.render(block).unwrap();
        }
        assert_eq!(actual, expected);
    }
    for frame in [
        0, 11999, 12000, 17999, 18000, 23999, 24000, 47999, 48000, 59999, 60000, 72000,
    ] {
        assert_eq!(
            preview.value_at(f.track, frame as u64),
            Some(expected_gain(frame))
        );
    }
    let mut segments = Vec::new();
    assert!(preview.visit_segments(f.track, 11000, 61000, |segment| segments.push(segment)));
    for segment in &segments {
        assert_eq!(
            segment.start_gain,
            expected_gain(segment.start_frame as usize)
        );
        for frame in segment.start_frame..segment.end_frame {
            let fraction = (frame - segment.start_frame) as f64
                / (segment.end_frame - segment.start_frame) as f64;
            let interpolated = (f64::from(segment.start_gain)
                + f64::from(segment.end_gain - segment.start_gain) * fraction)
                as f32;
            assert!((interpolated - expected_gain(frame as usize)).abs() < 2e-7);
        }
    }
    let last = segments.last().unwrap();
    assert_eq!(last.end_frame, 61000);
    assert!(
        last.end_gain > last.start_gain,
        "the original ramp resumes at the exact span end"
    );
}

#[test]
fn touch_steps_share_frames_later_beat_wins_end_returns_static_and_collapsed_span_is_silent() {
    let mut f = fixture(16);
    f.editor
        .execute(Command::Gain {
            track: f.track,
            gain: 0.6,
        })
        .unwrap();
    let end = f.editor.project().tempo_map().frame_to_beat(4).unwrap();
    splice(
        &mut f,
        0,
        span(
            BeatTime::ZERO,
            end,
            1,
            &[
                (BeatTime::ZERO, 0.1),
                (BeatTime::from_raw(1), 0.8),
                (BeatTime::from_raw(2), 1.2),
            ],
        ),
    );
    let start = f.editor.project().tempo_map().frame_to_beat(8).unwrap();
    splice(
        &mut f,
        1,
        span(
            start,
            BeatTime::from_raw(start.raw() + 1),
            2,
            &[(start, 2.0)],
        ),
    );
    let expected: Vec<_> = (0..16)
        .map(|frame| {
            let gain = if frame < 4 {
                ramp(0.6, 1.2, frame + 1)
            } else {
                ramp(ramp(0.6, 1.2, 4), 0.6, frame - 3)
            };
            [0.25 * gain, -0.5 * gain]
        })
        .collect();
    assert_eq!(arrangement(&f), expected);
    let r = revision(&f);
    assert_eq!(
        r.arrangement()
            .gain_automation_capacity()
            .prepared_touch_spans,
        1
    );
    assert_eq!(
        r.arrangement()
            .gain_automation_capacity()
            .prepared_touch_steps,
        1
    );
    let mut runtime = playback(&f);
    let mut actual = vec![[0.0; 2]; 16];
    for block in actual.chunks_mut(3) {
        runtime.render(block).unwrap();
    }
    assert_eq!(actual, expected);
    let preview = GainAutomationPreview::prepare(f.editor.project()).unwrap();
    assert_eq!(preview.value_at(f.track, 3), Some(ramp(0.6, 1.2, 4)));
    assert_eq!(
        preview.value_at(f.track, 4),
        Some(ramp(ramp(0.6, 1.2, 4), 0.6, 1))
    );
    assert_eq!(
        preview.value_at(f.track, 8),
        Some(ramp(ramp(0.6, 1.2, 4), 0.6, 5))
    );
}

#[test]
fn touch_gain_drives_track_and_post_fader_send_capture_once() {
    let mut f = fixture(32);
    let start = f.editor.project().tempo_map().frame_to_beat(3).unwrap();
    let end = f.editor.project().tempo_map().frame_to_beat(17).unwrap();
    splice(&mut f, 0, span(start, end, 1, &[(start, 0.8)]));
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.track,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let r = revision(&f);
    let mut runtime = SessionPlayback::new_linear_arrangement_export(r).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let plan = runtime
        .output_capture_plan(
            &[
                OutputCaptureId::Track(f.track),
                OutputCaptureId::Return(bus),
            ],
            32,
        )
        .unwrap();
    let mut main = [[0.0; 2]; 32];
    let mut track = main;
    let mut returned = main;
    let mut buffers = [&mut track[..], &mut returned[..]];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    runtime
        .render_with_capture(&mut main, &mut capture)
        .unwrap();
    for (frame, main_frame) in main.iter().enumerate() {
        let gain = if frame < 3 {
            0.0
        } else if frame < 17 {
            ramp(0.0, 0.8, frame - 2)
        } else {
            ramp(ramp(0.0, 0.8, 14), 0.0, frame - 16)
        };
        let expected = [0.25 * gain, -0.5 * gain];
        assert_eq!(capture.output(0).unwrap()[frame], expected);
        assert_eq!(capture.output(1).unwrap()[frame], expected.map(|v| v * 0.5));
        assert_eq!(*main_frame, expected.map(|v| v + v * 0.5));
    }
    assert_eq!(arrangement(&f), main);
}

#[test]
fn detached_effect_tail_freezes_touch_gain_and_current_splice_preserves_history() {
    let mut f = fixture(64);
    let state = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
            "hex.utility",
        ))
        .unwrap(),
        hex_lattice_contract::ParameterId::new(11).unwrap(),
        hex_lattice_contract::ParameterValue::Bool(true),
    )
    .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.track,
            before: None,
            state,
        })
        .unwrap();
    let end = f.editor.project().tempo_map().frame_to_beat(8).unwrap();
    splice(
        &mut f,
        0,
        span(BeatTime::ZERO, end, 1, &[(BeatTime::ZERO, 0.75)]),
    );
    let mut old = playback(&f);
    old.render(&mut [[0.0; 2]; 4]).unwrap();
    let mut snapshot = f.editor.project().snapshot();
    snapshot.tracks[0].gain_touch_spans.clear();
    snapshot.tracks[0].gain = ramp(0.0, 0.75, 5);
    let project = Project::from_snapshot(snapshot).unwrap();
    let mut samples = vec![[0.25, -0.5]; 64];
    samples[4..].fill([0.0; 2]);
    let mut oracle = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(
            &project,
            &BTreeMap::from([(f.asset, Audio::new(48000, samples).unwrap())]),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    oracle.render(&mut [[0.0; 2]; 4]).unwrap();
    let mut expected = [[0.0; 2]; 12];
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|value| value.abs() > 1e-6));
    f.editor.execute(Command::RemoveTrack(f.track)).unwrap();
    let mut next = SessionPlayback::new(revision(&f)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 12];
    next.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    assert!(f.editor.undo());
    let mut old = playback(&f);
    old.render(&mut [[0.0; 2]; 4]).unwrap();
    splice(
        &mut f,
        1,
        span(BeatTime::ZERO, end, 2, &[(BeatTime::ZERO, 0.3)]),
    );
    let mut oracle = playback(&f);
    oracle.render(&mut [[0.0; 2]; 4]).unwrap();
    oracle.render(&mut expected).unwrap();
    let mut next = SessionPlayback::new(revision(&f)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    next.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn session_source_launch_keeps_arrangement_touch_timing_instead_of_restarting_lane() {
    let mut f = fixture(32);
    let start = f.editor.project().tempo_map().frame_to_beat(2).unwrap();
    let end = f.editor.project().tempo_map().frame_to_beat(6).unwrap();
    splice(&mut f, 0, span(start, end, 1, &[(start, 0.8)]));
    let Created::Scene(scene) = f
        .editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = f
        .editor
        .execute(Command::CreateSessionAudioClip {
            track: f.track,
            scene,
            name: "session".into(),
            asset: f.asset,
            source_start: 0,
            source_end: 32,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let mut runtime = playback(&f);
    runtime.render(&mut [[0.0; 2]; 4]).unwrap();
    runtime
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut actual = [[0.0; 2]; 4];
    runtime.render(&mut actual).unwrap();
    let gains = [
        ramp(0.0, 0.8, 3),
        ramp(0.0, 0.8, 4),
        ramp(ramp(0.0, 0.8, 4), 0.0, 1),
        ramp(ramp(0.0, 0.8, 4), 0.0, 2),
    ];
    assert_eq!(actual, gains.map(|gain| [0.25 * gain, -0.5 * gain]));
}

#[test]
fn armed_live_override_is_transient_absolute_and_transport_clears_it() {
    use hex_engine::{PlaybackCommand, playback::Playback};
    let f = fixture(64);
    let before = f.editor.project().snapshot();
    let normal = revision(&f);
    assert_eq!(normal.gain_recording_track(), None);
    let armed = Arc::new(
        PreparedRevision::prepare_for_gain_recording(
            f.editor.project(),
            &media(&f),
            Arc::new(SineTable::new()),
            Some(normal.session()),
            f.track,
        )
        .unwrap(),
    );
    assert_eq!(armed.gain_recording_track(), Some(f.track));
    assert_eq!(f.editor.project().snapshot(), before);
    assert!(
        armed.arrangement().tracks()[0]
            .gain_envelope_knots()
            .is_empty(),
        "arming does not fabricate durable curve knots"
    );
    assert_eq!(
        armed.arrangement().tracks()[0].prepared_gain_at(0),
        Some(0.0)
    );
    let mut runtime = Playback::try_from_prepared_with_session(armed).unwrap();
    assert!(runtime.gain_recording_ready(f.track.value()));
    assert!(!runtime.gain_recording_ready(u64::MAX));
    assert_eq!(
        runtime.gain_recording_clock(f.track.value()),
        Some((0, 0, 0))
    );
    assert!(runtime.set_gain_recording_override(u64::MAX, 0.8).is_err());
    runtime.apply(PlaybackCommand::Play);
    let mut output = [[9.0; 2]; 4];
    runtime.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 4]);
    runtime
        .set_gain_recording_override(f.track.value(), 0.8)
        .unwrap();
    for gain in [f32::NAN, f32::INFINITY, -0.1, 2.1] {
        assert!(
            runtime
                .set_gain_recording_override(f.track.value(), gain)
                .is_err()
        );
    }
    runtime.render(&mut output).unwrap();
    assert_eq!(
        output,
        [1, 2, 3, 4].map(|n| {
            let gain = ramp(0.0, 0.8, n);
            [0.25 * gain, -0.5 * gain]
        })
    );
    runtime.apply(PlaybackCommand::Pause);
    runtime.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 4]);
    assert_eq!(
        runtime.gain_recording_clock(f.track.value()),
        Some((8, 0, 0))
    );
    runtime.apply(PlaybackCommand::Play);
    runtime.render(&mut output).unwrap();
    assert_eq!(
        output,
        [5, 6, 7, 8].map(|n| {
            let gain = ramp(0.0, 0.8, n);
            [0.25 * gain, -0.5 * gain]
        })
    );
    runtime.clear_gain_recording_override();
    runtime.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 4]);
    runtime
        .set_gain_recording_override(f.track.value(), 0.8)
        .unwrap();
    runtime.apply(PlaybackCommand::Seek(1));
    runtime.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 4]);
    runtime
        .set_gain_recording_override(f.track.value(), 0.8)
        .unwrap();
    runtime.apply(PlaybackCommand::Stop);
    runtime.apply(PlaybackCommand::Play);
    runtime.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 4]);
    let mut default = Playback::try_from_prepared_with_session(normal).unwrap();
    assert!(!default.gain_recording_ready(f.track.value()));
    assert!(
        default
            .set_gain_recording_override(f.track.value(), 0.5)
            .is_err()
    );
    assert_eq!(f.editor.project().snapshot(), before);
}

#[test]
fn clearing_live_override_resumes_saved_base_and_touch_law() {
    use hex_engine::{PlaybackCommand, playback::Playback};
    let mut f = fixture(64);
    lane(
        &mut f,
        vec![point(BeatTime::ZERO, 0.5, GainInterpolation::Step)],
    );
    let start = f.editor.project().tempo_map().frame_to_beat(4).unwrap();
    let end = f.editor.project().tempo_map().frame_to_beat(12).unwrap();
    splice(&mut f, 0, span(start, end, 1, &[(start, 0.75)]));
    let armed = Arc::new(
        PreparedRevision::prepare_for_gain_recording(
            f.editor.project(),
            &media(&f),
            Arc::new(SineTable::new()),
            None,
            f.track,
        )
        .unwrap(),
    );
    let mut runtime = Playback::try_from_prepared_with_session(armed).unwrap();
    runtime.apply(PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 4];
    runtime.render(&mut output).unwrap();
    assert_eq!(output, [[0.125, -0.25]; 4]);
    runtime
        .set_gain_recording_override(f.track.value(), 1.0)
        .unwrap();
    runtime.render(&mut output).unwrap();
    assert_eq!(
        output,
        [1, 2, 3, 4].map(|n| {
            let gain = ramp(0.5, 1.0, n);
            [0.25 * gain, -0.5 * gain]
        })
    );
    runtime.clear_gain_recording_override();
    runtime.render(&mut output).unwrap();
    assert_eq!(
        output,
        [5, 6, 7, 8].map(|elapsed| {
            let gain = ramp(0.5, 0.75, elapsed);
            [0.25 * gain, -0.5 * gain]
        })
    );
    runtime.render(&mut output).unwrap();
    assert_eq!(
        output,
        [1, 2, 3, 4].map(|elapsed| {
            let gain = ramp(ramp(0.5, 0.75, 8), 0.5, elapsed);
            [0.25 * gain, -0.5 * gain]
        })
    );
}

#[test]
fn armed_recording_clock_reports_actual_content_loop_pass() {
    use hex_engine::{PlaybackCommand, playback::Playback};
    let mut f = fixture(32);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 2,
            end: 4,
            enabled: true,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_for_gain_recording(
            f.editor.project(),
            &media(&f),
            Arc::new(SineTable::new()),
            None,
            f.track,
        )
        .unwrap(),
    );
    let mut runtime = Playback::try_from_prepared_with_session(revision).unwrap();
    runtime.apply(PlaybackCommand::Play);
    runtime.render(&mut [[0.0; 2]; 5]).unwrap();
    assert_eq!(
        runtime.gain_recording_clock(f.track.value()),
        Some((3, 0, 1))
    );
    runtime.render(&mut [[0.0; 2]; 3]).unwrap();
    assert_eq!(
        runtime.gain_recording_clock(f.track.value()),
        Some((2, 0, 3))
    );
    runtime.apply(PlaybackCommand::Seek(3));
    assert_eq!(
        runtime.gain_recording_clock(f.track.value()),
        Some((3, 0, 0))
    );
}

#[test]
fn detached_effect_tail_freezes_live_override_instead_of_static_zero() {
    use hex_engine::{PlaybackCommand, playback::Playback};
    let mut f = fixture(64);
    let state = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
            "hex.utility",
        ))
        .unwrap(),
        hex_lattice_contract::ParameterId::new(11).unwrap(),
        hex_lattice_contract::ParameterValue::Bool(true),
    )
    .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.track,
            before: None,
            state,
        })
        .unwrap();
    let armed = Arc::new(
        PreparedRevision::prepare_for_gain_recording(
            f.editor.project(),
            &media(&f),
            Arc::new(SineTable::new()),
            None,
            f.track,
        )
        .unwrap(),
    );
    let mut old = Playback::try_from_prepared_with_session(armed).unwrap();
    old.apply(PlaybackCommand::Play);
    old.set_gain_recording_override(f.track.value(), 0.8)
        .unwrap();
    old.render(&mut [[0.0; 2]; 4]).unwrap();
    let mut snapshot = f.editor.project().snapshot();
    snapshot.tracks[0].gain = ramp(0.0, 0.8, 4);
    let project = Project::from_snapshot(snapshot).unwrap();
    let mut samples = vec![[0.25, -0.5]; 64];
    samples[4..].fill([0.0; 2]);
    let mut oracle = Playback::try_from_prepared_with_session(Arc::new(
        PreparedRevision::prepare(
            &project,
            &BTreeMap::from([(f.asset, Audio::new(48000, samples).unwrap())]),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    oracle.apply(PlaybackCommand::Play);
    oracle.render(&mut [[0.0; 2]; 4]).unwrap();
    let mut expected = [[0.0; 2]; 12];
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|sample| sample.abs() > 1e-6));
    f.editor.execute(Command::RemoveTrack(f.track)).unwrap();
    let mut next = Playback::try_from_prepared_with_session(revision(&f)).unwrap();
    next.try_inherit_runtime_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 12];
    next.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    assert!(!next.gain_recording_ready(f.track.value()));
}

#[test]
fn section_splices_preserve_linear_left_limits_and_relocated_touch_provenance() {
    use hex_project::ElementColor;
    let mut f = fixture(300_000);
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    lane(
        &mut f,
        vec![
            point(beat(0), 0.2, GainInterpolation::Linear),
            point(beat(5), 1.6, GainInterpolation::Linear),
            point(beat(12), 0.8, GainInterpolation::Step),
        ],
    );
    splice(
        &mut f,
        0,
        span(
            beat(1),
            beat(9),
            1,
            &[(beat(1), 0.7), (beat(3), 0.4), (beat(8), 0.9)],
        ),
    );
    let mut add = |name: &str, start, end| {
        let hex_project::Created::ArrangementSection(id) = f
            .editor
            .execute(Command::AddArrangementSection {
                name: name.into(),
                color: ElementColor::Amber,
                start: beat(start),
                end: beat(end),
            })
            .unwrap()
        else {
            panic!("section")
        };
        id
    };
    let a = add("intro", 2, 4);
    let c = add("chorus", 7, 10);
    let old = f.editor.project().clone();
    let old_preview = GainAutomationPreview::prepare(&old).unwrap();
    let permutation = old.preview_section_reorder(c, a).unwrap();
    f.editor
        .execute(Command::ReorderSectionBefore {
            section: c,
            before: a,
        })
        .unwrap();
    let new = f.editor.project();
    let preview = GainAutomationPreview::prepare(new).unwrap();
    let near_touch_transition = |project: &Project, frame: u64| {
        project
            .track(f.track)
            .unwrap()
            .gain_lane()
            .spans()
            .iter()
            .any(|span| {
                std::iter::once(span.start)
                    .chain(span.steps.iter().map(|step| step.beat))
                    .chain(std::iter::once(span.end))
                    .any(|beat| {
                        let boundary = project.tempo_map().beat_to_frame(beat).unwrap();
                        frame >= boundary && frame < boundary.saturating_add(240)
                    })
            })
    };
    for numerator in 0..48 {
        let at = quarter(numerator, 4);
        let mapped = permutation.map_beat(at).unwrap();
        let before = old_preview
            .value_at(f.track, old.tempo_map().beat_to_frame(at).unwrap())
            .unwrap();
        let after = preview
            .value_at(f.track, new.tempo_map().beat_to_frame(mapped).unwrap())
            .unwrap();
        if near_touch_transition(&old, old.tempo_map().beat_to_frame(at).unwrap())
            || near_touch_transition(new, new.tempo_map().beat_to_frame(mapped).unwrap())
        {
            continue;
        }
        assert!(
            (after - before).abs() < 1e-6,
            "beat {numerator}/4: {before} vs {after}"
        );
    }
    let pcm = arrangement(&f);
    let compiled = revision(&f);
    // Probe the immediately adjacent samples at every cut, touch boundary and
    // ramp knot. These check actual gain rendering and half-open seam ownership.
    for source_beat in [1, 2, 3, 5, 7, 8, 9, 10, 12] {
        let boundary = old.tempo_map().beat_to_frame(beat(source_beat)).unwrap();
        for offset in [-2_i64, -1, 0, 1, 2] {
            let frame = boundary.checked_add_signed(offset).unwrap();
            let destination = if (168_000..240_000).contains(&frame) {
                48_000 + frame - 168_000
            } else if (48_000..168_000).contains(&frame) {
                frame + 72_000
            } else {
                frame
            };
            if near_touch_transition(&old, frame) || near_touch_transition(new, destination) {
                continue;
            }
            let expected = old_preview.value_at(f.track, frame).unwrap();
            let actual = compiled.arrangement().tracks()[0]
                .prepared_gain_at(destination)
                .unwrap();
            assert!(
                (actual - expected).abs() < 1e-6,
                "source frame {frame} mapped to {destination}"
            );
            assert!((pcm[destination as usize][0] - 0.25 * expected).abs() < 1e-6);
            assert!((pcm[destination as usize][1] + 0.5 * expected).abs() < 1e-6);
        }
    }
    let owner = new.track(f.track).unwrap();
    owner.gain_lane().validate().unwrap();
    assert!(
        owner
            .gain_lane()
            .spans()
            .windows(2)
            .any(|pair| pair[0].source_start > pair[1].source_start),
        "recording event chronology must remain original after placement changes"
    );
    assert!(
        owner.gain_envelope().iter().any(|p| p
            .incoming_gain
            .is_some_and(|left| (left - p.gain).abs() > 0.1)),
        "linear curve splice must retain an explicit jump"
    );
    assert_eq!(
        Project::from_snapshot(new.snapshot()).unwrap().snapshot(),
        new.snapshot()
    );
}
