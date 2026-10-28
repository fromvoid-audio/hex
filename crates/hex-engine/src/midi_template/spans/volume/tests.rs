use super::*;
use crate::{MidiInstrumentRenderContext, PreparedTrackMix};
use hex_project::{
    Command, Created, Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint, Project,
};

struct LiteralVoice;
impl PreparedNativeInstrument for LiteralVoice {
    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        let mut payload = [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[0] = u64::from((f32::from(velocity) / 127.0).to_bits());
        payload[1] = u64::from(pitch);
        Ok(PreparedAnalyticNote::new(payload, 4000, sample))
    }
}
fn sample(
    payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    age: u64,
    gate: u64,
    release: u64,
    _: &dyn hex_lattice::DeviceResources,
) -> [f32; 2] {
    if age >= gate + release {
        return [0.0; 2];
    }
    let value = f32::from_bits(payload[0] as u32);
    [value, -value * (payload[1] as f32 / 128.0)]
}
fn b(numerator: u64, denominator: u64) -> BeatTime {
    BeatTime::from_raw(BeatTime::ONE.raw() * numerator / denominator)
}
fn fixture() -> (
    PreparedTempoMap,
    SineTable,
    PreparedMidiInstrumentTemplate,
    PreparedMidiClipVolume,
) {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Clip(id) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: b(1, 1),
        })
        .unwrap()
    else {
        panic!("clip");
    };
    // An earlier note is not chased by cue. Two exact-cue voices and one later
    // voice all keep their own velocities, including their ordinary releases.
    for (start, pitch, velocity) in [
        (b(0, 1), 55, 127),
        (b(1, 2), 60, 37),
        (b(1, 2), 64, 93),
        (b(3, 4), 67, 75),
    ] {
        editor
            .execute(Command::AddMidiNote {
                clip: id,
                start,
                duration: b(1, 8),
                pitch,
                channel: 0,
                velocity,
            })
            .unwrap();
    }
    editor
        .execute(Command::SetMidiClipSource {
            clip: id,
            source_length: b(1, 1),
            source_start: b(1, 4),
            loop_start: BeatTime::ZERO,
            loop_end: b(1, 1),
            looping: false,
            length: b(1, 1),
        })
        .unwrap();
    let timing = PreparedTempoMap::prepare(editor.project().tempo_map().clone(), 8000).unwrap();
    let clip = editor.project().midi_clip(id).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(track, clip, &timing).unwrap();
    let instrument =
        PreparedMidiInstrumentTemplate::prepare_with_instrument(&source, &LiteralVoice).unwrap();
    let volume = MidiClipVolume::new(
        vec![
            MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.0).unwrap()),
            MidiClipVolumePoint {
                beat: b(1, 1),
                incoming: MidiClipGain::new(0.5).unwrap(),
                value: MidiClipGain::new(2.0).unwrap(),
            },
        ],
        b(1, 1),
    )
    .unwrap();
    let program =
        PreparedMidiClipVolume::prepare(&volume, clip.source_playback().unwrap(), &timing).unwrap();
    (timing, SineTable::new(), instrument, program)
}
fn render(
    instrument: &PreparedMidiInstrumentTemplate,
    timing: &PreparedTempoMap,
    table: &SineTable,
    program: Option<&PreparedMidiClipVolume>,
    partitions: &[usize],
) -> Vec<[f32; 2]> {
    let context = MidiInstrumentRenderContext::with_origin(
        SessionMidiOrigin::new(b(2, 1), b(1, 4)),
        timing,
        table,
    );
    let mix = PreparedTrackMix::new(0.7, 0.2, false).unwrap();
    let mut output = vec![[0.125, -0.25]; 3000];
    let mut buffer = MidiSpanBuffer::default();
    let mut offset = 0;
    for width in partitions.iter().cycle() {
        let end = (offset + width).min(output.len());
        instrument
            .render_additive_with_volume(
                context,
                mix,
                8000 + offset as u64,
                &mut output[offset..end],
                &mut buffer,
                program,
            )
            .unwrap();
        offset = end;
        if offset == output.len() {
            break;
        }
    }
    output
}

#[test]
fn analytic_session_loop_keeps_outgoing_and_new_voice_gains_independent() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip_id) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: b(2, 1),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: clip_id,
            start: BeatTime::ZERO,
            duration: b(1, 2),
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    editor
        .execute(Command::SetMidiClipSource {
            clip: clip_id,
            source_length: b(1, 1),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: b(1, 1),
            looping: true,
            length: b(2, 1),
        })
        .unwrap();
    let clip = editor.project().midi_clip(clip_id).unwrap();
    let timing = PreparedTempoMap::prepare(editor.project().tempo_map().clone(), 8000).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(track, clip, &timing).unwrap();
    let instrument =
        PreparedMidiInstrumentTemplate::prepare_with_instrument(&source, &LiteralVoice).unwrap();
    let curve = MidiClipVolume::new(
        vec![
            MidiClipVolumePoint::linear(b(0, 1), MidiClipGain::new(0.5).unwrap()),
            MidiClipVolumePoint::linear(b(1, 1), MidiClipGain::new(2.0).unwrap()),
        ],
        b(1, 1),
    )
    .unwrap();
    let layer = hex_project::MidiVoiceGainLayer::new(b(0, 1), b(1, 1), b(1, 1), curve).unwrap();
    let law = hex_project::MidiVoiceGain::new(vec![layer]).unwrap();
    let voices = crate::midi_clip_volume::voice::PreparedSessionVoiceGains::test_voice(
        clip.notes()[0].id(),
        clip.source_playback().unwrap(),
        &law,
        &timing,
    )
    .unwrap();
    let table = SineTable::new();
    let context = MidiInstrumentRenderContext::with_origin(
        SessionMidiOrigin::launch(BeatTime::ZERO),
        &timing,
        &table,
    );
    let mix = PreparedTrackMix::new(1.0, 0.0, false).unwrap();
    let run = |parts: &[usize], shared: Option<&PreparedMidiClipVolume>| {
        let mut output = vec![[0.0; 2]; 8000];
        let mut spans = MidiSpanBuffer::default();
        let mut offset = 0;
        for width in parts.iter().cycle() {
            let end = (offset + width).min(output.len());
            instrument
                .render_additive_with_voice_volume(
                    context,
                    mix,
                    offset as u64,
                    &mut output[offset..end],
                    &mut spans,
                    shared,
                    &voices,
                )
                .unwrap();
            offset = end;
            if offset == output.len() {
                break;
            }
        }
        output
    };
    let output = run(&[256], None);
    assert_eq!(output, run(&[1, 17, 63, 127], None));
    let shared_curve = MidiClipVolume::new(
        vec![MidiClipVolumePoint::linear(
            BeatTime::ZERO,
            MidiClipGain::new(0.25).unwrap(),
        )],
        b(1, 1),
    )
    .unwrap();
    let shared =
        PreparedMidiClipVolume::prepare(&shared_curve, clip.source_playback().unwrap(), &timing)
            .unwrap();
    let scaled = run(&[17, 256], Some(&shared));
    for (actual, original) in scaled.iter().zip(&output) {
        assert_eq!(*actual, [original[0] * 0.25, original[1] * 0.25]);
    }
    for (frame, sample) in output.iter().enumerate() {
        let mut expected = [0.0; 2];
        for start in [0, 4000] {
            if frame >= start && frame < start + 6000 {
                let age = frame - start;
                let gain = 0.5 + 1.5 * (age.min(4000) as f64 / 4000.0);
                mix.add_frame(
                    &mut expected,
                    [gain as f32, (-gain * (60.0 / 128.0)) as f32],
                );
            }
        }
        for channel in 0..2 {
            assert!(
                (sample[channel] - expected[channel]).abs() < 1e-6,
                "frame {frame}, channel {channel}: {} vs {}",
                sample[channel],
                expected[channel]
            );
        }
    }
    assert!(
        output[4000][0] > output[3999][0],
        "new voice must overlap the old release"
    );
    let other_timing = PreparedTempoMap::prepare(
        hex_project::TempoMap::constant(16_000, hex_project::Tempo::new(120_000_000).unwrap())
            .unwrap(),
        16_000,
    )
    .unwrap();
    let incompatible = crate::midi_clip_volume::voice::PreparedSessionVoiceGains::test_voice(
        clip.notes()[0].id(),
        clip.source_playback().unwrap(),
        &law,
        &other_timing,
    )
    .unwrap();
    let mut preserved = [[0.25, -0.25]; 16];
    let mut spans = MidiSpanBuffer::default();
    assert!(
        instrument
            .render_additive_with_voice_volume(
                context,
                mix,
                0,
                &mut preserved,
                &mut spans,
                None,
                &incompatible,
            )
            .is_err()
    );
    assert_eq!(preserved, [[0.25, -0.25]; 16]);
    assert!(spans.is_empty());
}
#[test]
fn analytic_chord_volume_matches_literal_per_voice_golden_and_uneven_partitions() {
    let (timing, table, instrument, program) = fixture();
    let actual = render(&instrument, &timing, &table, Some(&program), &[256]);
    assert_eq!(
        actual,
        render(
            &instrument,
            &timing,
            &table,
            Some(&program),
            &[1, 127, 13, 256, 7]
        )
    );
    let mix = PreparedTrackMix::new(0.7, 0.2, false).unwrap();
    let expected: Vec<_> = (0..3000)
        .map(|offset| {
            let gain = if offset < 2000 {
                0.25 + offset as f64 / 8000.0
            } else {
                0.5
            };
            let mut output = [0.125, -0.25];
            for (start, pitch, velocity) in [(0, 60, 37), (0, 64, 93), (1000, 67, 75)] {
                if offset < start {
                    continue;
                }
                let value = velocity as f32 / 127.0;
                let contribution = [value, -value * (pitch as f32 / 128.0)];
                mix.add_frame(
                    &mut output,
                    [
                        (f64::from(contribution[0]) * gain) as f32,
                        (f64::from(contribution[1]) * gain) as f32,
                    ],
                );
            }
            output
        })
        .collect();
    assert_eq!(
        actual, expected,
        "source offset/cue and naturally released voices use the same exact gain"
    );
    assert_ne!(actual[1999], actual[2000]);
    assert_eq!(
        actual[2000], actual[2999],
        "finite tail freezes incoming.5, never terminal right2"
    );
}
#[test]
fn unity_keeps_public_render_bits_and_silent_windows_do_not_query_missing_segments() {
    let (timing, table, instrument, program) = fixture();
    let unity_curve = MidiClipVolume::new(
        vec![MidiClipVolumePoint::linear(
            BeatTime::ZERO,
            MidiClipGain::UNITY,
        )],
        b(1, 1),
    )
    .unwrap();
    let unity = PreparedMidiClipVolume::prepare(
        &unity_curve,
        hex_project::MidiSourcePlayback::new(b(1, 1), b(1, 4), None, b(1, 1)).unwrap(),
        &timing,
    )
    .unwrap();
    let expected = render(&instrument, &timing, &table, None, &[256]);
    assert_eq!(
        expected,
        render(&instrument, &timing, &table, Some(&unity), &[13, 127, 256])
    );
    let context = MidiInstrumentRenderContext::with_origin(
        SessionMidiOrigin::new(b(2, 1), b(1, 4)),
        &timing,
        &table,
    );
    let mix = PreparedTrackMix::new(0.7, 0.2, false).unwrap();
    let mut public = vec![[0.125, -0.25]; 256];
    instrument
        .render_additive(
            context,
            mix,
            8000,
            &mut public,
            &mut MidiSpanBuffer::default(),
        )
        .unwrap();
    assert_eq!(public, expected[..256]);
    let other_clock = PreparedTempoMap::prepare(
        hex_project::TempoMap::constant(16000, hex_project::Tempo::new(120_000_000).unwrap())
            .unwrap(),
        16000,
    )
    .unwrap();
    let mismatched = PreparedMidiClipVolume::prepare(
        &unity_curve,
        hex_project::MidiSourcePlayback::new(b(1, 1), b(1, 4), None, b(1, 1)).unwrap(),
        &other_clock,
    )
    .unwrap();
    let mut preserved = [[0.125, -0.25]; 10];
    let mut refused_spans = MidiSpanBuffer::default();
    assert!(matches!(
        instrument.render_additive_with_volume(
            context,
            mix,
            8000,
            &mut preserved,
            &mut refused_spans,
            Some(&mismatched)
        ),
        Err(MidiTemplateError::InvalidSource(
            "clip volume timing exceeds admission"
        ))
    ));
    assert_eq!(
        preserved,
        [[0.125, -0.25]; 10],
        "volume preflight refuses before touching output"
    );
    assert!(refused_spans.is_empty());
    for start in [7990, 14000] {
        let mut output = [[0.125, -0.25]; 10];
        instrument
            .render_additive_with_volume(
                context,
                mix,
                start,
                &mut output,
                &mut MidiSpanBuffer::default(),
                Some(&program),
            )
            .unwrap();
        assert_eq!(output, [[0.125, -0.25]; 10]);
    }
    let rebased = crate::prepared_tempo::AnchoredSessionTempoView::new(
        crate::prepared_tempo::SessionBaseTimingView::Canonical(&timing),
        crate::SessionClockAnchor {
            cut_frame: 4000,
            old_beat: b(7, 1),
            new_beat: b(1, 1),
        },
    );
    let mut output = [[0.125, -0.25]; 10];
    instrument
        .render_additive_with_volume(
            MidiInstrumentRenderContext::with_origin(
                SessionMidiOrigin::launch(b(10, 1)),
                &rebased,
                &table,
            ),
            mix,
            4050,
            &mut output,
            &mut MidiSpanBuffer::default(),
            Some(&program),
        )
        .unwrap();
    assert_eq!(
        output,
        [[0.125, -0.25]; 10],
        "silent pre-origin spans do not map obsolete source endpoints"
    );
}
