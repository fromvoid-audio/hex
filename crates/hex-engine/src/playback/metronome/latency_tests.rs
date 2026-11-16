//! Click alignment with Main output latency.
use crate::playback::Playback;
use crate::{Frame, PlaybackAction, PlaybackCommand, PreparedRevision, SineTable};
use hex_project::{LoopRegion, Project};
use std::{collections::BTreeMap, sync::Arc};

/// A 48 kHz project whose Main limiter adds latency, with a one-sample
/// transient on beat 1 (frame 24,000 at 120 bpm).
fn main_latency_revision(loop_region: Option<LoopRegion>) -> (Arc<PreparedRevision>, u64) {
    main_latency_revision_at_tempo(loop_region, 120_000_000, 24_000)
}

fn main_latency_revision_at_tempo(
    loop_region: Option<LoopRegion>,
    micro_bpm: u32,
    beat_one_frame: u64,
) -> (Arc<PreparedRevision>, u64) {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(micro_bpm).unwrap(),
        })
        .unwrap();
    let Created::Imported { track, asset, clip } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "transient".into(),
            path: "memory/transient.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 4_096,
            playback_frames: 4_096,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: beat_one_frame,
        })
        .unwrap();
    let definition = &hex_devices::limiter::DEFINITION;
    let state = definition
        .patch_parameter(
            &definition.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(2),
        )
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    if let Some(region) = loop_region {
        editor
            .execute(Command::SetLoopRegion {
                start: region.start(),
                end: region.end(),
                enabled: true,
            })
            .unwrap();
    }
    let mut transient = vec![[0.0; 2]; 4_096];
    transient[0] = [0.25; 2];
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([(asset, crate::Audio::new(48_000, transient).unwrap())]),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let latency = u64::from(revision.arrangement().main_latency_frames());
    (revision, latency)
}

fn render(revision: &Arc<PreparedRevision>, click: bool, count: usize, block: usize) -> Vec<Frame> {
    let mut playback = Playback::try_from_prepared_with_session(Arc::clone(revision)).unwrap();
    playback
        .try_apply_action(PlaybackAction::SetMetronome(click))
        .unwrap();
    playback.apply(PlaybackCommand::Play);
    let mut output = vec![[0.0; 2]; count];
    for chunk in output.chunks_mut(block) {
        playback.render(chunk).unwrap();
    }
    output
}

/// Main output with the click on, minus the same output with it off. The click
/// is mixed after Main processing, so the difference is exactly the click.
fn click_and_music(
    revision: &Arc<PreparedRevision>,
    count: usize,
    block: usize,
) -> (Vec<f32>, Vec<f32>) {
    let with_click = render(revision, true, count, block);
    let music = render(revision, false, count, block);
    let click = with_click
        .iter()
        .zip(&music)
        .map(|(with, without)| with[0] - without[0])
        .collect();
    (click, music.iter().map(|frame| frame[0]).collect())
}

/// Frames where the signal first exceeds `threshold` after at least `gap`
/// frames below it.
fn onsets(signal: &[f32], threshold: f32, gap: usize) -> Vec<u64> {
    let mut onsets = Vec::new();
    let mut last_loud: Option<usize> = None;
    for (index, sample) in signal.iter().enumerate() {
        if sample.abs() > threshold {
            if last_loud.is_none_or(|last| index - last > gap) {
                onsets.push(index as u64);
            }
            last_loud = Some(index);
        }
    }
    onsets
}

#[test]
fn click_is_delayed_by_main_latency_to_meet_the_beat_aligned_transient() {
    const BEAT_ONE: u64 = 4_800;
    let (revision, latency) = main_latency_revision_at_tempo(None, 600_000_000, BEAT_ONE);
    assert_eq!(latency, 288);
    for block in [1, 63, 256, 1_024] {
        let (click, music) = click_and_music(&revision, 6_000, block);
        assert_eq!(
            onsets(&music, 0.01, 2_000),
            vec![BEAT_ONE + latency],
            "block {block}"
        );
        // Beat 0 and beat 1 clicks sound exactly with the delayed music.
        assert_eq!(
            onsets(&click, 0.05, 2_000),
            vec![latency, BEAT_ONE + latency],
            "block {block}"
        );
        assert!(
            click[..latency as usize]
                .iter()
                .all(|sample| *sample == 0.0)
        );
    }
}

#[test]
fn project_capture_timing_is_scheduled_not_latency_adjusted() {
    const BEAT_ONE: u64 = 4_800;
    let (revision, latency) = main_latency_revision_at_tempo(None, 600_000_000, BEAT_ONE);
    for block in [63, 256] {
        let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        playback.apply(PlaybackCommand::Play);
        let mut output = vec![[0.0; 2]; 6_000];
        let mut clock = vec![None; output.len()];
        for (index, part) in output.chunks_mut(block).enumerate() {
            let offset = index * block;
            let mut observe = |_, _, _: &[Frame]| {};
            let mut timing = |start: usize, frame: u64, len: usize, _: &crate::PreparedTempoMap| {
                for i in 0..len {
                    clock[offset + start + i] = Some(frame + i as u64);
                }
            };
            let mut tap = crate::ProjectOutputTap::new(&[], &mut observe)
                .unwrap()
                .with_timing(&mut timing);
            playback
                .render_with_timed_transport_and_signal_observers(part, &[], None, Some(&mut tap))
                .unwrap();
        }
        let audible = (BEAT_ONE + latency) as usize;
        assert!(output[audible][0] > 0.01);
        // ProjectOutputTap reports the render cursor, not the earlier content
        // heard after Main's processor delay. Recording placement must account
        // for this without changing sample capture's existing timing contract.
        assert_eq!(clock[audible], Some(BEAT_ONE + latency), "block {block}");
    }
}

#[test]
fn delayed_click_keeps_loop_wrap_pulses_and_its_in_flight_tail() {
    // Wrap 100 frames after beat 1, so the beat-1 click is still inside the
    // delay ring when the content clock wraps back to beat 0.
    let wrap = 24_100;
    let (revision, latency) = main_latency_revision(Some(LoopRegion::new(0, wrap, true).unwrap()));
    let (click, _) = click_and_music(&revision, 50_000, 256);
    // The loop-start pulse follows the in-flight beat-1 click with no gap, so
    // it is checked by its own first sample below.
    assert_eq!(
        onsets(&click, 0.05, 2_000),
        vec![latency, 24_000 + latency, wrap + 24_000 + latency]
    );
    // The wrap cuts the undelayed beat-1 click after 100 frames, but the 100
    // samples already in the delay ring still sound, exactly as without a loop.
    let (unlooped, _) = click_and_music(&main_latency_revision(None).0, 26_000, 256);
    let beat_one = (24_000 + latency) as usize;
    assert_eq!(
        click[beat_one..beat_one + 100],
        unlooped[beat_one..beat_one + 100]
    );
    // The accented loop-start pulse then sounds exactly `latency` after the wrap.
    let retrigger = (wrap + latency) as usize;
    assert!(
        (click[retrigger] - 0.16).abs() < 1e-6,
        "{}",
        click[retrigger]
    );
    assert!((click[retrigger] - unlooped[retrigger]).abs() > 0.01);
    let (single, _) = click_and_music(&revision, 50_000, 50_000);
    assert_eq!(single, click, "partition invariant");
}
