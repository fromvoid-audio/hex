//! Real signed scratch preparation alongside an applied authored effect.
use super::*;
use crate::plugin_effects::{Hosting, ManualBaseCapture};
use hex_plugin_host::transport::ParameterPoint;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_manual_base_capture_preserves_authored_pcm_and_recovers_scratch() {
    if std::env::var_os("HEX_ORDINARY_MANUAL_BASE_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_ORDINARY_MANUAL_BASE_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "144")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
        return;
    }

    let _serial = serial();
    let mut f = fixture_with_authored("processing", 2, 8, &[7]);
    let canonical = f.editor.project().clone();
    let initial = f.control.native_source_prepared_state(f.binding).unwrap();
    let (build, _, preparation) = crate::candidate_custody::native_source_test_fixture::input_for(
        "processing",
        [
            0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
        ],
    );
    let host = Hosting::from_test_build(build, preparation.limits);
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    let revision = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let source = AppliedAudioSource {
        context: context(),
        audio_revision: revision,
        output_identity: f.control.output_identity().unwrap(),
    };
    let point = ParameterPoint {
        id: 7,
        frame: 0,
        normalized: 0.75,
    };
    let binding = f.binding;
    assert!(
        host.begin_manual_base_capture_on_control(&mut f.control, source, binding, &initial, point)
            .is_err()
    );
    f.control.command(PlaybackCommand::Play).unwrap();
    assert_eq!(block(&mut f), [[0.0; 2]; 8]);
    replacement::wait_for_blocks(&root, binding, 1);
    assert_eq!(f.control.status.native_effect_source(), Some(source));
    let stale = AppliedAudioSource {
        audio_revision: revision + 1,
        ..source
    };
    let wrong_binding = Binding {
        epoch: binding.epoch + 1,
        ..binding
    };
    for (source, binding, point) in [
        (stale, binding, point),
        (source, wrong_binding, point),
        (source, binding, ParameterPoint { frame: 1, ..point }),
    ] {
        assert!(
            host.begin_manual_base_capture_on_control(
                &mut f.control,
                source,
                binding,
                &initial,
                point
            )
            .is_err()
        );
    }
    let start = |f: &mut OrdinaryFixture, state, point| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match host.begin_manual_base_capture_on_control(
                &mut f.control,
                source,
                binding,
                state,
                point,
            ) {
                Ok(capture) => break capture,
                Err(error)
                    if matches!(
                        error.as_str(),
                        "native preparation is busy" | "native source is busy"
                    ) => {}
                Err(error) => panic!("scratch refused: {error}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    };

    let mut abandoned = start(&mut f, &initial, point);
    assert!(poll(&mut f.control, &mut abandoned, true).is_err());
    assert!(
        abandoned
            .poll_on_control(&mut f.control, &mut || false)
            .is_none()
    );
    assert_eq!(block(&mut f), [[0.0; 2]; 8]);
    replacement::wait_for_blocks(&root, binding, 2);

    let mut refused = start(&mut f, &initial, ParameterPoint { id: 63, ..point });
    assert!(poll(&mut f.control, &mut refused, false).is_err());

    let mut scratch = start(&mut f, &initial, point);
    let captured = poll(&mut f.control, &mut scratch, false).unwrap();
    assert!(
        host.begin_manual_base_capture_on_control(
            &mut f.control,
            source,
            binding,
            &captured,
            point
        )
        .is_err()
    );
    let captured = hex_plugin_state::decode(&captured).unwrap();
    let original = hex_plugin_state::decode(&initial).unwrap();
    assert_eq!(captured.identity, original.identity);
    assert_ne!(captured.component.unwrap(), original.component.unwrap());
    assert_eq!(&captured.component.unwrap()[8..12], &3.0_f32.to_le_bytes());
    assert_eq!(
        f.control.native_source_prepared_state(binding),
        Some(initial.clone())
    );
    assert_eq!(f.control.status.native_effect_source(), Some(source));

    // Playback remains on the live authored 0.25 normalized lane throughout.
    assert_eq!(block(&mut f), [[0.25, -0.5]; 8]);
    replacement::wait_for_blocks(&root, binding, 3);
    assert_eq!(block(&mut f), [[0.25, -0.5]; 8]);
    replacement::wait_for_blocks(&root, binding, 4);
    assert_eq!(f.editor.project(), &canonical);
    dispose(f);
}

fn poll(
    output: &mut PublicationControl,
    capture: &mut ManualBaseCapture,
    cancelled: bool,
) -> Result<NativeDeviceState, String> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(terminal) = capture.poll_on_control(output, &mut || cancelled) {
            return terminal;
        }
        assert!(Instant::now() < until, "scratch capture did not dispose");
        std::thread::sleep(Duration::from_millis(1));
    }
}
