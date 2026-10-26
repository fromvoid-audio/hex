//! Actual source-bound SDK parameter processing through the production Rust
//! reader. Direct worker exchanges are not Root/callback delivery acceptance.
use super::*;
use crate::transport::{ParameterPoint, Parameters};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_parameter_pcm_feedback_and_capture_keep_original_source_with_no_caller_allocations() {
    for quantum in [1, 7, 64, 256] {
        let case = Case::new();
        let mut configured = options();
        configured.preparation.quantum = quantum;
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            configured,
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(37),
        )
        .unwrap();
        let child = instance.child_id_on_control();
        let initial = instance.ready().initial_state.clone();
        for n in 0..3 {
            let mut input = request(instance.configuration(), n);
            if n < 2 {
                input
                    .parameters
                    .push(ParameterPoint {
                        frame: if n == 0 { 0 } else { quantum - 1 },
                        id: 7,
                        normalized: if n == 0 { 0.25 } else { 0.75 },
                    })
                    .unwrap();
            }
            #[cfg(feature = "allocation-probe")]
            let result = {
                let mut result = None;
                let measured = allocation_counter::measure(|| {
                    result = Some(instance.exchange_on_worker(input));
                });
                assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
                assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
                result.unwrap()
            };
            #[cfg(not(feature = "allocation-probe"))]
            let result = instance.exchange_on_worker(input);
            let output = result.unwrap();
            assert_eq!(
                (output.binding, output.sequence, output.first_frame),
                (input.binding, n, n * u64::from(quantum))
            );
            for frame in 0..usize::from(quantum) {
                let normalized = match n {
                    0 => 0.25,
                    1 => 0.25 + 0.5 * (frame + 1) as f64 / f64::from(quantum),
                    _ => 0.75,
                };
                for channel in 0..2 {
                    let expected = f64::from(input.pcm[frame][channel])
                        * (0.5 + n as f64 * 0.125)
                        * normalized
                        * 4.0;
                    assert!(
                        (f64::from(output.pcm[frame][channel]) - expected).abs() < 1e-6,
                        "q{quantum}, block{n}, frame{frame}, channel{channel}"
                    );
                }
            }
            let mut expected = Parameters::default();
            if n < 2 {
                expected
                    .push(ParameterPoint {
                        frame: quantum - 1,
                        id: 7,
                        normalized: if n == 0 { 0.25 } else { 0.75 },
                    })
                    .unwrap();
            }
            assert_eq!(output.parameters, expected);
            assert_eq!(instance.child_id_on_control(), child);
            assert_eq!(instance.ready().initial_state, initial);
        }
        instance.begin_state_capture_on_worker().unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        let captured = loop {
            if let Some(state) = instance.poll_state_capture_on_worker().unwrap() {
                break state;
            }
            assert!(Instant::now() < until, "actual parameter-state capture");
            thread::yield_now();
        };
        assert_eq!(
            &decode(&captured).unwrap().component.unwrap()[8..12],
            &3.0_f32.to_le_bytes()
        );
        assert_eq!(
            &decode(&initial).unwrap().component.unwrap()[8..12],
            &1.0_f32.to_le_bytes()
        );
        assert_eq!(instance.child_id_on_control(), child);
        let terminal = instance
            .stop_and_reap_on_control()
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(
            (terminal.blocks, terminal.frames),
            (3, u64::from(quantum) * 3)
        );
    }
}
