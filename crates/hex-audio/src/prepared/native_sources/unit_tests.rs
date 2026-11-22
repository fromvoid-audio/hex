//! Fast non-SDK unit tests for `prepared::native_sources`.
use super::*;

#[test]
fn stopped_frontier_preserves_only_actual_quantum_and_empty_receipt() {
    use crate::plugin_worker::CommittedQuantumState;
    use hex_engine::ForeignProgramFrontier as Frontier;
    let key = std::num::NonZeroU64::new(7).unwrap();
    for (quantum, expected_key) in [
        (CommittedQuantumState::Closed, None),
        (CommittedQuantumState::OpenUnprogrammed, None),
        (CommittedQuantumState::OpenProgram(key), Some(key)),
    ] {
        for worker in [Frontier::Unknown, Frontier::Empty, Frontier::Program(key)] {
            let observed = hex_plugin_host::transport::QuantumObservation::Committed(
                crate::plugin_worker::CommittedQuantum {
                    binding: Binding {
                        root: 1,
                        instance: 2,
                        graph: 3,
                        epoch: 4,
                    },
                    state: quantum,
                },
            );
            assert_eq!(
                stopped_program_frontier(observed, worker),
                Frontier::Stopped {
                    open_quantum: expected_key,
                    worker_empty: worker == Frontier::Empty,
                }
            );
        }
    }
    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    assert_eq!(
        stopped_program_frontier(
            hex_plugin_host::transport::QuantumObservation::Untouched(binding),
            Frontier::Unknown
        ),
        Frontier::Stopped {
            open_quantum: None,
            worker_empty: false
        }
    );
    assert_eq!(
        stopped_program_frontier(
            hex_plugin_host::transport::QuantumObservation::Untouched(binding),
            Frontier::Empty
        ),
        Frontier::Stopped {
            open_quantum: None,
            worker_empty: true
        }
    );
}

#[test]
fn committed_quantum_is_unavailable_without_native_custody_or_after_closing() {
    let (mut control, _callback) = channel(48_000).unwrap();
    let binding = Binding {
        root: 1,
        instance: 1,
        graph: 1,
        epoch: 1,
    };
    assert!(
        control
            .publication
            .try_native_source_committed_quantum_on_control(binding)
            .is_err()
    );
    control.publication.enable_native_custody().unwrap();
    control
        .publication
        .status
        .closing
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        control
            .publication
            .try_native_source_committed_quantum_on_control(binding),
        Err("native automation output is closing")
    );
}
