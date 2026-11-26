//! Native worker to manually pumped callback, not a physical device deadline.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::Command;

#[test]
fn solo_publication_reaches_callback_at_the_audible_cut() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(48_000, 65_536);
    let mut h = ResidentHarness::with_frame_limit(131_072);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(path));
    (applied, _) = h.applied(applied);
    let audible = imported.state.project.tracks()[0].id();
    let added = h.run(Request::Edit(Command::AddTrack {
        name: "silent".into(),
    }));
    (applied, _) = h.applied(applied);
    let solo = added.state.project.tracks().last().unwrap().id();
    for index in 0..18 {
        h.run(Request::Edit(Command::AddTrack {
            name: format!("empty {index}"),
        }));
        (applied, _) = h.applied(applied);
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    assert!(h.render().iter().flatten().any(|sample| sample.abs() > 0.0));
    let mut times = Vec::new();
    for iteration in 0..22 {
        let expected = iteration % 2 == 0;
        let started = Instant::now();
        let event = h.run(Request::Edit(Command::ToggleSolo {
            track: solo,
            exclusive: true,
        }));
        let deadline = Instant::now() + Duration::from_millis(500);
        let (next, metadata) = loop {
            h.pump();
            if let Some((next, metadata)) = h.submitted.filter(|(next, _)| *next > applied) {
                break (next, metadata);
            }
            assert!(Instant::now() < deadline, "solo publication was not staged");
            std::thread::yield_now();
        };
        // The 5 ms track fader spans 240 samples at 48 kHz. A muting
        // predecessor keeps ownership through its fade. The unmuting
        // successor can install first and fade its track up from silence.
        const FADE: usize = 240;
        let mut installed_at = None;
        for block in 0..(FADE.div_ceil(64) + 1) {
            let pcm = h.render();
            let revision = h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire);
            if revision == next {
                installed_at.get_or_insert_with(|| started.elapsed().as_micros());
            } else {
                assert_eq!(revision, applied);
                assert!(installed_at.is_none(), "solo publication regressed");
                assert!(expected && block < FADE.div_ceil(64), "late solo cut");
            }
            for (offset, frame) in pcm.into_iter().enumerate() {
                let progress = ((block * 64 + offset + 1).min(FADE) as f32) / FADE as f32;
                let level = 0.25 * if expected { 1.0 - progress } else { progress };
                assert!(
                    frame.iter().all(|sample| (sample - level).abs() < 1e-5),
                    "solo={expected} block={block} frame={offset}: {frame:?}, expected {level}"
                );
            }
        }
        let elapsed = installed_at.expect("solo successor must install at the audible cut");
        if iteration >= 2 {
            times.push(elapsed);
        }
        applied = next;
        assert_eq!(metadata.context.editor_revision, event.state.revision);
        assert_eq!(event.state.project.track(solo).unwrap().soloed(), expected);
        assert!(!event.state.project.track(audible).unwrap().soloed());
    }
    times.sort_unstable();
    eprintln!(
        "solo worker-to-callback: 20 tracks (one audio, 19 empty), 48 kHz, 64-frame manually pumped blocks, 2 warmups, 20 samples, no recovery or device; p50_us={} p95_us={} max_us={}",
        (times[9] + times[10]) / 2,
        times[18],
        times[19]
    );
}
