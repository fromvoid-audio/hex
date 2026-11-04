//! Native worker to manually pumped callback, not a physical device deadline.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::Command;

#[test]
fn solo_publication_reaches_next_pumped_callback() {
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
        let output = h.render();
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire),
            next,
            "the first callback after staging must install the solo graph"
        );
        if iteration >= 2 {
            times.push(started.elapsed().as_micros());
        }
        applied = next;
        assert_eq!(metadata.context.editor_revision, event.state.revision);
        assert_eq!(event.state.project.track(solo).unwrap().soloed(), expected);
        assert!(!event.state.project.track(audible).unwrap().soloed());
        if expected {
            assert!(output.iter().flatten().all(|sample| sample.abs() < 1e-6));
        } else {
            assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
        }
    }
    times.sort_unstable();
    eprintln!(
        "solo worker-to-callback: 20 tracks (one audio, 19 empty), 48 kHz, 64-frame manually pumped blocks, 2 warmups, 20 samples, no recovery or device; p50_us={} p95_us={} max_us={}",
        (times[9] + times[10]) / 2,
        times[18],
        times[19]
    );
}
