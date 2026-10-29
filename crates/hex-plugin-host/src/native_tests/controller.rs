//! Actual no-editor SDK controller data; no native Hex application is started.
use super::*;

fn wait_controller_held(case: &Case) {
    let until = Instant::now() + Duration::from_secs(2);
    while case.observation().is_none_or(|values| values[0] != 26) {
        assert!(
            Instant::now() < until,
            "actual SDK formatter did not enter hold"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn held_controller_query_keeps_pcm_running_and_cancels_without_sdk_teardown() {
    for cancel in [false, true] {
        let case = Case::new();
        let cancellation = PreparationCancellation::new_on_control().unwrap();
        let token = cancellation.token_on_control();
        let mut instance =
            Prepared::prepare(&build(), source(), options(), cancellation, case.setup(93)).unwrap();
        let child = instance.child_id_on_control();
        let initial = instance.ready().initial_state.clone();
        instance
            .begin_parameter_page_on_worker(ParameterPageQuery::default())
            .unwrap();
        wait_controller_held(&case);
        for n in 0..4 {
            assert!(matches!(
                instance.poll_parameter_page_on_worker().unwrap(),
                ParameterPageProgress::Pending
            ));
            assert!(
                instance
                    .begin_parameter_page_on_worker(ParameterPageQuery::default())
                    .is_err()
            );
            assert!(instance.stop_and_reap_on_control().is_err());
            let input = request(instance.configuration(), n);
            let output = instance.exchange_on_worker(input).unwrap();
            for (actual, original) in output.pcm[..8].iter().zip(&input.pcm[..8]) {
                let scale = 0.5 + n as f32 * 0.125;
                assert_eq!(*actual, [original[0] * scale, original[1] * scale]);
            }
        }
        assert_eq!(instance.child_id_on_control(), child);
        assert_eq!(instance.completed_blocks_on_control(), 4);
        assert_eq!(instance.ready().initial_state, initial);
        assert_eq!(
            case.observation().unwrap()[9],
            0,
            "no SDK stop under getter"
        );
        if cancel {
            token.cancel_on_control();
            assert!(matches!(
                instance.poll_parameter_page_on_worker(),
                Err(Error::Cancelled)
            ));
            let retired = instance.cancel_and_reap_on_control().unwrap();
            assert!(!retired.status.success() && retired.terminal.is_none());
            assert!(
                !case.release().exists(),
                "guardian retires held source without releasing getter"
            );
            let observed = case.observation().unwrap();
            assert_eq!((observed[1], observed[4], observed[9]), (0, 0, 0));
            assert!(instance.child_id_on_control().is_none());
        } else {
            fs::write(case.release(), b"release actual SDK formatter").unwrap();
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                match instance.poll_parameter_page_on_worker().unwrap() {
                    ParameterPageProgress::Page(page) => {
                        assert_eq!(page.as_slice()[0].id, 7);
                        break;
                    }
                    ParameterPageProgress::Pending => {}
                    ParameterPageProgress::Changed { .. } => panic!("unversioned first query"),
                    ParameterPageProgress::Conversion(_) => panic!("expected metadata page"),
                }
                assert!(Instant::now() < until);
                thread::sleep(Duration::from_millis(1));
            }
            let retired = instance.stop_and_reap_on_control().unwrap();
            assert!(retired.status.success());
            assert_eq!(retired.terminal.unwrap().blocks, 4);
            assert_eq!(case.observation().unwrap()[1], 0);
        }
    }
}
use crate::protocol::{
    ParameterConversion, ParameterConversionResult, ParameterConversionStatus, ParameterPage,
    ParameterPageProgress, ParameterPageQuery, ParameterPlainValue, ParameterText,
};

fn converted(
    mut poll: impl FnMut() -> crate::Result<ParameterPageProgress>,
) -> Box<ParameterConversionResult> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match poll().unwrap() {
            ParameterPageProgress::Pending => {}
            ParameterPageProgress::Conversion(value) => return value,
            other => panic!("actual SDK conversion terminal: {other:?}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}
fn convert(
    instance: &mut Prepared,
    conversion: ParameterConversion,
) -> Box<ParameterConversionResult> {
    instance
        .begin_parameter_page_on_worker(ParameterPageQuery {
            conversion: Some(conversion),
            ..ParameterPageQuery::default()
        })
        .unwrap();
    converted(|| instance.poll_parameter_page_on_worker())
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn sdk_text_and_plain_conversions_preserve_live_values_pcm_and_stopped_source() {
    for variant in [91, 92, 80] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(variant),
        )
        .unwrap();
        let child = instance.child_id_on_control();
        let initial = instance.ready().initial_state.clone();
        let before = page(&mut instance, ParameterPageQuery::default());
        let id = if variant == 91 { 7 } else { 0 };
        let parsed = convert(
            &mut instance,
            ParameterConversion::Parse {
                id,
                text: ParameterText::from_text("double").unwrap(),
            },
        );
        if variant == 80 {
            assert_eq!(parsed.status, ParameterConversionStatus::Unsupported);
            assert_eq!(parsed.normalized, None);
            let value = convert(
                &mut instance,
                ParameterConversion::Format {
                    id,
                    normalized: 0.5,
                },
            );
            assert_eq!(value.normalized, Some(0.5));
            assert_eq!(value.plain, ParameterPlainValue::Finite(0.5));
            assert_eq!(value.display, None);
        } else {
            assert_eq!(parsed.status, ParameterConversionStatus::Converted);
            assert_eq!(parsed.normalized, Some(0.5));
            assert_eq!(parsed.plain, ParameterPlainValue::Finite(2.0));
            assert_eq!(
                parsed.display.unwrap(),
                ParameterText::from_text("double").unwrap()
            );
            let value = convert(
                &mut instance,
                ParameterConversion::Format {
                    id,
                    normalized: 0.8125,
                },
            );
            assert_eq!(value.normalized, Some(0.8125));
            assert_eq!(value.plain, ParameterPlainValue::Finite(3.25));
            assert_eq!(
                value.display.unwrap(),
                ParameterText::from_text("3.250 x").unwrap()
            );
            let value = convert(
                &mut instance,
                ParameterConversion::FromPlain {
                    id,
                    plain: ParameterPlainValue::Finite(3.0),
                },
            );
            assert_eq!(value.normalized, Some(0.75));
            assert_eq!(value.plain, ParameterPlainValue::Finite(3.0));
            let value = convert(
                &mut instance,
                ParameterConversion::Parse {
                    id,
                    text: ParameterText::from_text("invalid text").unwrap(),
                },
            );
            assert_eq!(value.status, ParameterConversionStatus::Refused);
            assert_eq!(value.normalized, None);
            let value = convert(
                &mut instance,
                ParameterConversion::FromPlain {
                    id,
                    plain: ParameterPlainValue::NegativeInfinity,
                },
            );
            assert_eq!(value.status, ParameterConversionStatus::Refused);
            assert_eq!(value.normalized, None);
        }
        let missing = convert(
            &mut instance,
            ParameterConversion::Parse {
                id: 999,
                text: ParameterText::from_text("double").unwrap(),
            },
        );
        assert_eq!(missing.status, ParameterConversionStatus::Missing);
        assert_eq!(missing.normalized, None);
        let after = page(&mut instance, ParameterPageQuery::default());
        assert_eq!(
            *before, *after,
            "SDK conversion is not a setter or metadata edit"
        );
        assert_eq!(instance.ready().initial_state, initial);
        assert_eq!(instance.completed_blocks_on_control(), 0);
        let input = request(instance.configuration(), 0);
        let output = instance.exchange_on_worker(input).unwrap();
        for (output, input) in output.pcm[..8].iter().zip(&input.pcm[..8]) {
            assert_eq!(*output, [input[0] * 0.5, input[1] * 0.5]);
        }
        let mut stopped = instance.quiesce_retained_on_control().unwrap();
        stopped
            .begin_parameter_page_on_control(ParameterPageQuery {
                conversion: Some(ParameterConversion::Format {
                    id,
                    normalized: 0.5,
                }),
                ..ParameterPageQuery::default()
            })
            .unwrap();
        let value = converted(|| stopped.poll_parameter_page_on_control());
        assert_eq!(value.normalized, Some(0.5));
        assert_eq!(stopped.child_id_on_control(), child);
        assert_eq!(stopped.completed_blocks(), 1);
        assert_eq!(
            (
                case.observation().unwrap()[10],
                case.observation().unwrap()[11]
            ),
            (1, 0)
        );
        assert!(stopped.retire_on_control().unwrap().status.success());
        assert_eq!(case.observation().unwrap()[1], 0);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn malformed_sdk_parse_success_retires_original_source_without_fabricated_value() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(94),
    )
    .unwrap();
    let initial = instance.ready().initial_state.clone();
    instance
        .begin_parameter_page_on_worker(ParameterPageQuery {
            conversion: Some(ParameterConversion::Parse {
                id: 7,
                text: ParameterText::from_text("malformed sdk result").unwrap(),
            }),
            ..ParameterPageQuery::default()
        })
        .unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match instance.poll_parameter_page_on_worker() {
            Ok(ParameterPageProgress::Pending) => {}
            Err(Error::Worker(11)) => break,
            other => panic!("invalid SDK success must refuse source: {other:?}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    assert_eq!(instance.ready().initial_state, initial);
    assert_eq!(instance.completed_blocks_on_control(), 0);
    let retired = instance.cancel_and_reap_on_control().unwrap();
    assert!(!retired.status.success() && retired.terminal.is_none());
    assert!(instance.child_id_on_control().is_none());
}

fn page(instance: &mut Prepared, query: ParameterPageQuery) -> Box<ParameterPage> {
    instance.begin_parameter_page_on_worker(query).unwrap();
    assert!(instance.begin_parameter_page_on_worker(query).is_err());
    assert!(instance.stop_and_reap_on_control().is_err());
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match instance.poll_parameter_page_on_worker().unwrap() {
            ParameterPageProgress::Page(page) => return page,
            ParameterPageProgress::Pending => {}
            ParameterPageProgress::Changed { .. } => panic!("unchanged fixture metadata"),
            ParameterPageProgress::Conversion(_) => panic!("expected metadata page"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_controller_pages_use_sdk_conversions_without_processing_or_replacing_source() {
    for variant in [91, 92] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(variant),
        )
        .unwrap();
        let child = instance.child_id_on_control();
        let initial = instance.ready().initial_state.clone();
        let first = page(&mut instance, ParameterPageQuery::default());
        assert_eq!(
            (first.first, first.total, first.as_slice().len()),
            (0, 1, 1)
        );
        let value = first.as_slice()[0];
        let id = if variant == 92 { 0 } else { 7 };
        assert_eq!(value.id, id);
        assert!(value.is_editable());
        assert_eq!((value.normalized, value.default_normalized), (0.25, 0.25));
        assert_eq!(value.plain, ParameterPlainValue::Finite(1.0));
        assert_eq!(value.default_plain, ParameterPlainValue::Finite(1.0));
        assert_eq!(
            value.title.as_utf16(),
            "gain".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(
            value.units.as_utf16(),
            "ratio".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(
            value.display.unwrap().as_utf16(),
            "unity".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(
            value.default_display.unwrap().as_utf16(),
            "unity".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(instance.completed_blocks_on_control(), 0);
        let observed = case.observation().unwrap();
        assert_eq!((observed[10], observed[11]), (0, 0));

        let mut input = request(instance.configuration(), 0);
        input
            .parameters
            .push(crate::transport::ParameterPoint {
                frame: 0,
                id,
                normalized: 0.5,
            })
            .unwrap();
        let output = instance.exchange_on_worker(input).unwrap();
        assert_eq!(&output.pcm[..8], &input.pcm[..8]); // SDK gain2 * independent block0 scale0.5.
        let current = page(&mut instance, ParameterPageQuery::default());
        assert!(current.revision > first.revision);
        let query = ParameterPageQuery {
            first: 0,
            revision: current.revision,
            conversion: None,
        };
        assert_eq!(current.as_slice()[0].normalized, 0.5);
        assert_eq!(
            current.as_slice()[0].plain,
            ParameterPlainValue::Finite(2.0)
        );
        assert_eq!(
            current.as_slice()[0].display.unwrap().as_utf16(),
            "double".encode_utf16().collect::<Vec<_>>()
        );

        instance
            .begin_parameter_page_on_worker(ParameterPageQuery {
                revision: first.revision,
                ..query
            })
            .unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match instance.poll_parameter_page_on_worker().unwrap() {
                ParameterPageProgress::Pending => {}
                ParameterPageProgress::Changed { revision, total } => {
                    assert_eq!((revision, total), (current.revision, 1));
                    break;
                }
                ParameterPageProgress::Page(_) | ParameterPageProgress::Conversion(_) => {
                    panic!("stale metadata must invalidate enumeration")
                }
            }
            assert!(Instant::now() < until);
            thread::yield_now();
        }
        let mut stopped = instance.quiesce_retained_on_control().unwrap();
        stopped.begin_parameter_page_on_control(query).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        let stopped_page = loop {
            match stopped.poll_parameter_page_on_control().unwrap() {
                ParameterPageProgress::Page(page) => break page,
                ParameterPageProgress::Pending => {}
                ParameterPageProgress::Changed { .. } => panic!("same stopped metadata"),
                ParameterPageProgress::Conversion(_) => panic!("expected stopped metadata page"),
            }
            assert!(Instant::now() < until);
            thread::yield_now();
        };
        assert_eq!(stopped_page.as_slice(), current.as_slice());
        assert_eq!(stopped.child_id_on_control(), child);
        assert_eq!(stopped.completed_blocks(), 1);
        assert_eq!(
            (
                case.observation().unwrap()[10],
                case.observation().unwrap()[11]
            ),
            (1, 0)
        );
        assert_eq!(
            &decode(&initial).unwrap().component.unwrap()[8..12],
            &1.0_f32.to_le_bytes()
        );
        assert!(stopped.retire_on_control().unwrap().status.success());
        assert_eq!(case.observation().unwrap()[1], 0);
    }
}
