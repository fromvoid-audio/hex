//! Original receipt-channel retirement; no SDK/native output is claimed here.
use super::*;
use crate::foreign_parameter_delivery::{Receipt, RetiredOutcome};
use hex_plugin_host::transport::ManualParameterAction;
fn counted(action: impl FnOnce()) {
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(action);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    action();
}
#[test]
fn detached_original_receipt_survives_publication_drop_before_or_after_final_delivery() {
    for early in [false, true] {
        for release in [false, true] {
            let (mut prepared, prepared_callback) = channel(48_000).unwrap();
            let (delivery, mut callback, target, mut marker) =
                crate::foreign_parameter_delivery::tests::fixture();
            if release {
                marker.input.action = ManualParameterAction::Release {
                    parameter: target.parameter,
                };
            }
            let publication = &mut prepared.publication;
            publication.status.reserve_native_parameter_input().unwrap();
            publication.foreign_parameter_delivery = Some(delivery);
            let source = Some((target.source, target.transport_epoch));
            if early {
                counted(|| callback.finish(Some(marker), source, 131, 30, true, true));
            }
            let mut reader = publication
                .retire_native_source_parameter_delivery_on_control()
                .unwrap();
            assert_eq!(reader.target(), target);
            assert!(
                publication
                    .retire_native_source_parameter_delivery_on_control()
                    .is_none()
            );
            assert_eq!(
                publication
                    .begin_native_source_parameter_change_with_delivery_on_control(
                        target.source,
                        target.binding,
                        0,
                        0.5
                    )
                    .err(),
                Some("vst3 delivery reader is retired")
            );
            drop(prepared);
            drop(prepared_callback);
            if !early {
                assert_eq!(reader.try_take_on_control(), None);
                counted(|| callback.finish(Some(marker), source, 131, 30, true, true));
            }
            let observed = reader.try_take_on_control().unwrap();
            let expected = if release {
                Receipt::Released {
                    target,
                    origin: marker.origin.unwrap(),
                    delivery_performance_frame: 130,
                }
            } else {
                Receipt::Delivered {
                    target,
                    normalized: marker.input.point().unwrap().normalized,
                    origin: marker.origin.unwrap(),
                    delivery_performance_frame: 130,
                }
            };
            assert_eq!(observed, RetiredOutcome::Observed(expected));
            assert_eq!(reader.try_take_on_control(), None);
            drop(callback);
            assert_eq!(reader.try_take_on_control(), None);
        }
    }
}
#[test]
fn abandoned_producer_retains_its_last_actual_receipt_or_reports_interruption_once() {
    for delivered in [false, true] {
        let (mut prepared, prepared_callback) = channel(48_000).unwrap();
        let (delivery, mut callback, target, marker) =
            crate::foreign_parameter_delivery::tests::fixture();
        prepared
            .publication
            .status
            .reserve_native_parameter_input()
            .unwrap();
        prepared.publication.foreign_parameter_delivery = Some(delivery);
        let mut reader = prepared
            .publication
            .retire_native_source_parameter_delivery_on_control()
            .unwrap();
        drop(prepared);
        drop(prepared_callback);
        assert_eq!(reader.try_take_on_control(), None);
        if delivered {
            counted(|| {
                callback.finish(
                    Some(marker),
                    Some((target.source, target.transport_epoch)),
                    131,
                    30,
                    true,
                    true,
                )
            });
        }
        drop(callback);
        let result = reader.try_take_on_control().unwrap();
        if delivered {
            assert!(
                matches!(result,RetiredOutcome::Observed(Receipt::Delivered {target:actual,..}) if actual==target)
            );
        } else {
            assert_eq!(result, RetiredOutcome::Interrupted { target });
        }
        assert_eq!(reader.try_take_on_control(), None);
    }
}
