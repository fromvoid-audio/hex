//! Arrangement loop edits made while playing reach the running callback.
use super::resident_preview::ResidentHarness;
use super::*;

fn position(h: &ResidentHarness) -> u64 {
    h.audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire)
}

/// Render `blocks` callbacks of constant source PCM; return the visited span.
fn play(h: &ResidentHarness, blocks: usize) -> (u64, u64) {
    let (mut low, mut high) = (u64::MAX, 0);
    for _ in 0..blocks {
        assert!(
            h.render()
                .iter()
                .flatten()
                .all(|v| (*v - 0.25).abs() < 1e-5),
            "uninterrupted audio near {}",
            position(h)
        );
        let at = position(h);
        (low, high) = (low.min(at), high.max(at));
    }
    (low, high)
}

fn set_loop(h: &mut ResidentHarness, applied: u64, start: u64, end: u64) -> u64 {
    h.run(Request::Edit(hex_project::Command::SetLoopRegion {
        start,
        end,
        enabled: true,
    }));
    h.applied(applied).0
}

#[test]
fn loop_edits_while_playing_a_compensated_track_follow_the_playhead() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    (applied, _) = h.applied(applied);
    // Lookahead latency gives the graph a compensated main clock.
    h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
        before: None,
    });
    (applied, _) = h.applied(applied);
    applied = set_loop(&mut h, applied, 0, 16_384);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..12 {
        h.render();
    }
    play(&h, 88);
    assert!((5_000..8_192).contains(&position(&h)), "{}", position(&h));

    // Inside: shrinking the loop around the playhead wraps at the new end.
    applied = set_loop(&mut h, applied, 0, 8_192);
    let (_, high) = play(&h, 300);
    assert!(high < 8_192, "old loop end still playing: {high}");

    // Before: a loop moved ahead of the playhead is entered, then repeats.
    applied = set_loop(&mut h, applied, 12_288, 20_480);
    play(&h, 400);
    let (low, high) = play(&h, 300);
    assert!(low >= 12_288 && high < 20_480, "new loop {low}..{high}");

    // After: a loop moved behind the playhead lets playback continue.
    let before = position(&h);
    set_loop(&mut h, applied, 0, 4_096);
    let (low, _) = play(&h, 1);
    assert!(low > before, "playhead jumped back from {before} to {low}");
    let (_, high) = play(&h, 300);
    assert!(high > 20_480 + 8_192, "linear playback stopped at {high}");
}
