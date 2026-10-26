//! Public live capture through actual ordinary graph and installed endpoint.
use super::*;
use crate::plugin_worker::{NativeWorkerCapture, NativeWorkerCaptureProgress};

fn begin(f: &mut OrdinaryFixture) -> NativeWorkerCapture {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match f
            .control
            .begin_native_source_state_capture_on_control(f.binding)
        {
            Ok(request) => return request,
            Err("native preparation is busy" | "native capture mailbox is busy") => {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            Err(reason) => panic!("live capture admission: {reason}"),
        }
    }
}
fn captured(f: &mut OrdinaryFixture, request: &NativeWorkerCapture) -> NativeDeviceState {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match f
            .control
            .poll_native_source_state_capture_on_control(f.binding, request)
        {
            Ok(NativeWorkerCaptureProgress::Captured(state)) => return state,
            Ok(NativeWorkerCaptureProgress::Pending) | Err("native preparation is busy") => {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            other => panic!("live capture terminal: {other:?}"),
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_track_return_main_live_capture_keeps_graph_pcm_and_canonical_state() {
    let _serial = serial();
    for location in 0..3 {
        let mut f = fixture_at("processing", location);
        let canonical = f.editor.project().clone();
        let initial = f.control.native_source_prepared_state(f.binding).unwrap();
        let root = f.control.native_candidate_root();
        let playback = compile(&f, true);
        publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        f.control.command(PlaybackCommand::Play).unwrap();
        block(&mut f);
        replacement::wait_for_blocks(&root, f.binding, 1);
        assert_eq!(
            f.control.native_source_endpoint_status(f.binding),
            Some((true, true))
        );
        let request = begin(&mut f);
        for n in 1..4_u64 {
            let output = block(&mut f);
            replacement::wait_for_blocks(&root, f.binding, n + 1);
            let gain =
                0.5 + n.saturating_sub(2) as f32 * 0.125 + if location == 1 { 1.0 } else { 0.0 };
            assert_eq!(
                output,
                if n < 2 {
                    [[0.0; 2]; 8]
                } else {
                    [[0.25 * gain, -0.5 * gain]; 8]
                }
            );
        }
        let current = captured(&mut f, &request);
        assert_ne!(current, initial);
        assert_eq!(
            f.control.native_source_prepared_state(f.binding),
            Some(initial)
        );
        assert_eq!(f.editor.project(), &canonical);
        assert_eq!(
            f.control.native_source_endpoint_status(f.binding),
            Some((true, true))
        );
        drop(request);
        dispose(f);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_live_capture_held_sdk_keeps_installed_source_and_pcm() {
    if std::env::var_os("HEX_ROOT_LIVE_CAPTURE_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let path =
            std::env::temp_dir().join(format!("hex-root-live-capture-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env("HEX_ROOT_LIVE_CAPTURE_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "71")
            .env("HEX_ORIGINAL_INSTANCE_TRACE", path.join("trace"))
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", path.join("release"))
            .output()
            .unwrap();
        std::fs::remove_dir_all(path).unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _serial = serial();
    let mut f = fixture("processing");
    let canonical = f.editor.project().clone();
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    block(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 1);
    let applied = f.control.status.native_effect_source().unwrap();
    let request = begin(&mut f);
    let until = Instant::now() + Duration::from_secs(2);
    let trace = std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap();
    while std::fs::read(&trace)
        .ok()
        .is_none_or(|data| data.get(..4) != Some(&22_i32.to_ne_bytes()))
    {
        assert!(Instant::now() < until, "actual SDK capture hold");
        std::thread::yield_now();
    }
    for n in 1..4_u64 {
        assert!(matches!(
            f.control
                .poll_native_source_state_capture_on_control(f.binding, &request)
                .unwrap(),
            NativeWorkerCaptureProgress::Pending
        ));
        let output = block(&mut f);
        replacement::wait_for_blocks(&root, f.binding, n + 1);
        let gain = 0.5 + n.saturating_sub(2) as f32 * 0.125;
        assert_eq!(
            output,
            if n < 2 {
                [[0.0; 2]; 8]
            } else {
                [[0.25 * gain, -0.5 * gain]; 8]
            }
        );
        assert_eq!(f.control.status.native_effect_source(), Some(applied));
    }
    std::fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"release",
    )
    .unwrap();
    captured(&mut f, &request);
    assert_eq!(f.editor.project(), &canonical);
    assert_eq!(
        f.control.native_source_endpoint_status(f.binding),
        Some((true, true))
    );
    drop(request);
    dispose(f);
}
