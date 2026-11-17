//! Arrangement loop edits made while playing reach the running callback.
use super::resident_preview::ResidentHarness;
use super::*;

/// Verify constant audio and the exact 128-frame Arrangement loop-end fade.
/// The limiter delays the output, so follow the delivered PCM rather than
/// deriving its source position from the current transport position.
pub(super) struct LoopFadeVerifier {
    previous: Option<i32>,
    saw_fade: bool,
    saw_attack: bool,
}

impl LoopFadeVerifier {
    pub(super) fn new() -> Self {
        Self {
            previous: None,
            saw_fade: false,
            saw_attack: false,
        }
    }

    pub(super) fn check(&mut self, frames: &[hex_engine::Frame]) {
        for frame in frames {
            for sample in frame {
                let scaled = *sample * (127.0 / 0.25);
                let step = scaled.round() as i32;
                assert!(
                    (0..=127).contains(&step) && (scaled - step as f32).abs() < 0.006,
                    "unexpected loop PCM: {sample}"
                );
            }
            assert!(
                (frame[0] - frame[1]).abs() < 1e-6,
                "stereo mismatch: {frame:?}"
            );
            let step = (frame[0] * (127.0 / 0.25)).round() as i32;
            if let Some(previous) = self.previous {
                let valid = match previous {
                    127 => step == 127 || step == 126,
                    0 => step == 127,
                    _ => step == previous - 1,
                };
                assert!(valid, "invalid loop fade step: {previous} -> {step}");
                self.saw_fade |= previous == 1 && step == 0;
                self.saw_attack |= previous == 0 && step == 127;
            }
            self.previous = Some(step);
        }
    }

    pub(super) fn assert_wrap(&self) {
        assert!(
            self.saw_fade && self.saw_attack,
            "expected a complete fade and full-level loop attack"
        );
    }
}

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
    let mut fade = LoopFadeVerifier::new();
    for _ in 0..blocks {
        fade.check(&h.render());
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
fn scope_resize_waits_for_the_previous_graph_and_returns_a_terminal_on_timeout() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(65_536);
    h.next();
    let (applied, _) = h.applied(0);
    h.run(Request::Import(files.wav(48_000, 32_768)));
    let (applied, _) = h.applied(applied);
    let applied = set_loop(&mut h, applied, 0, 16_384);
    let pending = h.run(Request::Edit(hex_project::Command::MasterGain {
        gain: 0.5,
    }));
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.publication_in_flight());
    assert_eq!(
        status.applied_revision.load(Acquire),
        applied,
        "leave the earlier graph queued in the actual callback",
    );
    let resize = || {
        Request::Edit(hex_project::Command::SetLoopRegion {
            start: 0,
            end: 8_192,
            enabled: true,
        })
    };

    h.service.submit(resize()).unwrap();
    let refused = h.next();
    assert_eq!(
        refused.error.as_deref(),
        Some("edit cancelled · audio publication timed out")
    );
    assert_eq!(refused.state.revision, pending.state.revision);
    assert_eq!(refused.state.project, pending.state.project);

    h.service.submit(resize()).unwrap();
    let (previous, _) = h.applied(applied);
    let edited = h.next();
    assert!(edited.error.is_none(), "{:?}", edited.error);
    assert_eq!(edited.state.revision, pending.state.revision + 1);
    assert_eq!(
        edited.state.project.loop_region(),
        Some(hex_project::LoopRegion::new(0, 8_192, true).unwrap())
    );
    // A toggle uses the same wait while the resized graph is still queued.
    h.service
        .submit(Request::Edit(hex_project::Command::ToggleLoopRegion))
        .unwrap();
    let (resized, metadata) = h.applied(previous);
    assert_eq!(metadata.context.editor_revision, edited.state.revision);
    let toggled = h.next();
    assert!(toggled.error.is_none(), "{:?}", toggled.error);
    assert_eq!(
        toggled.state.project.loop_region(),
        Some(hex_project::LoopRegion::new(0, 8_192, false).unwrap())
    );
    let (_, metadata) = h.applied(resized);
    assert_eq!(metadata.context.editor_revision, toggled.state.revision);
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
