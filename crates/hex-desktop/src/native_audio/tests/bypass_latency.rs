//! Resident worker publication to a manually pumped callback; no physical device.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{Command, Created};

#[test]
fn utility_bypass_publication_reaches_next_pumped_callback() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 65_536)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(6.0)
    .unwrap();
    let inserted = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state,
    }));
    let Some(Created::Device(device)) = inserted.created else {
        panic!("utility device")
    };
    (applied, _) = h.applied(applied);
    for index in 0..19 {
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
    let enabled = h.render()[63][0];
    assert!(enabled > 0.4, "non-unity Utility output: {enabled}");
    let mut times = Vec::new();
    for iteration in 0..22 {
        let bypassed = iteration % 2 == 0;
        let started = Instant::now();
        let event = h.run(Request::Edit(Command::SetDeviceBypass { device, bypassed }));
        let deadline = Instant::now() + Duration::from_millis(500);
        let (next, metadata) = loop {
            h.pump();
            if let Some((next, metadata)) = h.submitted.filter(|(next, _)| *next > applied) {
                break (next, metadata);
            }
            assert!(
                Instant::now() < deadline,
                "bypass publication was not staged"
            );
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
            next
        );
        assert_eq!(metadata.context.editor_revision, event.state.revision);
        assert_eq!(
            event.state.project.device(device).unwrap().bypassed(),
            bypassed
        );
        let expected = if bypassed { 0.25 } else { enabled };
        assert!(
            (output[63][0] - expected).abs() < 1e-4,
            "bypassed={bypassed} output={:?}",
            output[63]
        );
        if iteration >= 2 {
            times.push(started.elapsed().as_micros());
        }
        applied = next;
    }
    times.sort_unstable();
    eprintln!(
        "utility bypass worker-to-callback: 20 tracks, one audio and one +6 dB Utility, 48 kHz, 64-frame manually pumped blocks, 2 warmups, 20 samples, no recovery or physical device; p50_us={} p95_us={} max_us={}",
        (times[9] + times[10]) / 2,
        times[18],
        times[19]
    );
}
