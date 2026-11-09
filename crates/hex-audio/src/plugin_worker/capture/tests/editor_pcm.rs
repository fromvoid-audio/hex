//! Actual SDK editor, original worker, and fragmented accepted PCM.
use super::*;
use hex_plugin_host::{
    protocol::ParameterEditKind,
    transport::{EffectSegmentOrigins, ManualParameterOrigin, SegmentTiming},
};
use std::num::NonZeroU64;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn original_editor_perform_is_delivered_once_with_its_exact_pcm() {
    const NAME: &str = "plugin_worker::capture::tests::editor_pcm::original_editor_perform_is_delivered_once_with_its_exact_pcm";
    if fixture_child(89, NAME) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let scope = NonZeroU64::MIN;
    let mut render = |position: u64, len: usize| {
        let mut pcm = [[0.0; 2]; 8];
        let mut marker = None;
        let mut stage = || {
            endpoint.begin_physical_callback(len).unwrap();
            endpoint
                .stage_effect_segment(
                    0,
                    &[[0.5, -0.25]; 8][..len],
                    &[],
                    &[],
                    &mut pcm[..len],
                    SegmentTiming {
                        playing: None,
                        at: |_| None,
                    },
                    EffectSegmentOrigins {
                        authored: None,
                        manual: Some(&|index| {
                            let frame = position + index as u64;
                            Some(ManualParameterOrigin {
                                arrangement_frame: 20 + frame % 8,
                                performance_frame: 48 + frame,
                                loop_pass: frame / 8,
                            })
                        }),
                        dense: None,
                    },
                )
                .unwrap();
            endpoint.finish(true).unwrap();
            marker = endpoint.take_accepted_editor_quantum();
            assert!(endpoint.take_accepted_editor_quantum().is_none());
        };
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(&mut stage);
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        stage();
        (pcm, marker)
    };
    let wait = |owner: &NativeWorker, completed| {
        let deadline = Instant::now() + Duration::from_secs(3);
        while owner.published_blocks_on_control() < completed {
            assert!(
                Instant::now() < deadline,
                "{:?}",
                owner.take_failure_diagnostic_on_control()
            );
            thread::yield_now();
        }
    };
    assert!(render(0, 3).1.is_none());
    let arm = owner.arm_editor_pcm_on_control(scope).unwrap();
    assert!(render(3, 5).1.is_none());
    wait(&owner, 1);
    fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"editor",
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut edits = Vec::new();
    loop {
        match owner.poll_parameter_edits_on_control() {
            Ok(Some(batch)) => {
                edits.extend_from_slice(batch.as_slice());
                if batch.ready > 0 {
                    break;
                }
            }
            Ok(None) | Err("native edit observation is busy") => {}
            Err(error) => panic!("{error}"),
        }
        assert!(Instant::now() < deadline, "editor input was not queued");
        thread::yield_now();
    }
    assert!(render(8, 5).1.is_none());
    assert!(render(13, 3).1.is_none());
    wait(&owner, 2);
    assert!(render(16, 6).1.is_none());
    let (pcm, marker) = render(22, 4);
    let marker = marker.expect("the first exact processed sample was accepted");
    assert_eq!(
        (
            marker.binding,
            marker.sequence,
            marker.first_frame,
            marker.callback_offset,
            marker.scope
        ),
        (owner.binding(), 1, 8, 2, scope)
    );
    assert_eq!(
        marker.origin,
        Some(ManualParameterOrigin {
            arrangement_frame: 20,
            performance_frame: 56,
            loop_pass: 1,
        })
    );
    let [point] = marker.performs.as_slice() else {
        panic!("exactly one raw-zero input was applied");
    };
    assert_eq!((point.id, point.normalized), (0, 0.75));
    let gain = 3.0 * 0.625;
    assert_eq!(pcm[2..4], [[0.5 * gain, -0.25 * gain]; 2]);
    loop {
        match owner.poll_parameter_edits_on_control() {
            Ok(Some(batch)) => edits.extend_from_slice(batch.as_slice()),
            Ok(None) | Err("native edit observation is busy") => {}
            Err(error) => panic!("{error}"),
        }
        if let Some(edit) = edits
            .iter()
            .find(|edit| edit.kind == ParameterEditKind::Perform)
        {
            assert_eq!(
                (
                    edit.sequence,
                    edit.gesture,
                    edit.id,
                    edit.normalized,
                    edit.first_frame
                ),
                (
                    point.sequence,
                    point.gesture,
                    point.id,
                    point.normalized,
                    marker.first_frame
                )
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "ordered reader lost the original Perform"
        );
        thread::yield_now();
    }
    drop(arm);
    assert!(owner.retire_on_control().unwrap().native.status.success());
}
