//! Exercise the real Session request and private VST3 worker startup/return.
use super::*;
use hex_audio::{input_capture::MonitorStream, prepared::input_monitor::MonitorStatus};
use hex_session::service::{PausedInputMonitorRequest, ResidentGraphSubmission};

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn session_rearms_stopped_vst3_monitor_with_fresh_private_workers() {
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddTrack {
        name: "input".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("armed audio track");
    };
    let (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &added.state,
            PluginEffectDestination::Track(track),
        )))
        .unwrap();
    let accepted = terminal(&mut h);
    assert!(
        accepted.error.is_none() && accepted.plugin_effect_complete[0].accepted,
        "{:?}",
        accepted.error
    );
    h.applied(applied);
    let (source, transport_epoch) = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .paused_input_source()
        .unwrap();
    let request = PausedInputMonitorRequest {
        source: ResidentGraphSubmission {
            output_identity: source.output_identity,
            audio_revision: source.audio_revision,
            context: source.context,
        },
        track,
        transport_epoch,
    };
    let mut previous = None;
    for _ in 0..2 {
        let (reply, receive) = mpsc::sync_channel(1);
        h.service
            .submit(Request::PreparePausedInputMonitor { request, reply })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let monitor = loop {
            h.pump();
            h.render();
            match receive.try_recv() {
                Ok(ready) => break ready.unwrap(),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => panic!("monitor request ended"),
            }
            assert!(Instant::now() < deadline, "monitor endpoint ACK timed out");
            std::thread::yield_now();
        };
        let private = monitor.foreign_effect_bindings().next().unwrap();
        let (_, original, completed) =
            h.plugin_progress.lock().unwrap().source_bindings[0].unwrap();
        assert_eq!(private.instance, original.instance);
        assert_ne!(private, original);
        assert_ne!(Some(private), previous);
        assert_eq!(completed, Some(0));
        previous = Some(private);

        let (mut input, feed, ready) = hex_audio::worker_fixture::monitor_only_input().unwrap();
        let stream = Box::new(MonitorStream::new_on_control(feed, 48_000).ok().unwrap());
        assert!(input.start_on_control());
        wait(|| ready.started());
        assert!(
            h.audio
                .as_mut()
                .unwrap()
                .control
                .input_monitor()
                .unwrap()
                .publish_paused(stream, monitor, source, transport_epoch)
                .is_ok()
        );
        let first = loop {
            let pcm = h.render_plugin_block();
            let status = h
                .audio
                .as_mut()
                .unwrap()
                .control
                .input_monitor()
                .unwrap()
                .status();
            if status == MonitorStatus::Running {
                break pcm;
            }
            assert_eq!(status, MonitorStatus::Priming);
            assert!(Instant::now() < deadline, "monitor input did not prime");
            std::thread::yield_now();
        };
        assert_eq!(first, [[0.0; 2]; 256]);
        for expected in [[0.0; 2], [0.125; 2]] {
            wait(|| {
                h.callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .native_workers_idle_on_control()
                    == Some(true)
            });
            let pcm = h.render_plugin_block();
            assert_eq!(pcm, [expected; 256]);
        }
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .paused_input_source(),
            Some((source, transport_epoch))
        );
        h.audio
            .as_mut()
            .unwrap()
            .control
            .input_monitor()
            .unwrap()
            .stop();
        input.stop_on_control();
        assert_eq!(h.render_plugin_block(), [[0.0; 2]; 256]);
        let control = h.audio.as_mut().unwrap().control.input_monitor().unwrap();
        assert_eq!(control.status(), MonitorStatus::Stopped);
        assert!(control.reclaim().is_some());
    }
}
