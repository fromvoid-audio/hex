//! Actual no-editor SDK controller data; no native Hex application is started.
use super::*;

#[path = "controller/extension.rs"]
mod extension;

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
                    ParameterPageProgress::Conversion(_) | ParameterPageProgress::Editor(_) => {
                        panic!("expected metadata page")
                    }
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
            ParameterPageProgress::Conversion(_) | ParameterPageProgress::Editor(_) => {
                panic!("expected metadata page")
            }
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn large_controller_inventory_pages_pcm_and_saved_state_remain_bounded() {
    const INVENTORY: u16 = 2_593;
    let start = |case: &Case, inventory: &'static str, state| {
        let setup = case.setup_restore(37);
        Pending::prepare_with_restore(
            &build(),
            source(),
            options().preparation,
            state,
            PreparationCancellation::new_on_control().unwrap(),
            move |command| {
                setup(command);
                command.env("HEX_ORIGINAL_PARAMETER_INVENTORY", inventory);
            },
        )
        .and_then(|pending| pending.complete_on_control(Some(options().presentation)))
    };
    let case = Case::new();
    let mut instance = start(&case, "2593", None).unwrap();
    let first = page(&mut instance, ParameterPageQuery::default());
    assert_eq!((first.total, first.as_slice()[0].id), (INVENTORY, 1001));
    let last_page = |instance: &mut Prepared, revision| {
        page(
            instance,
            ParameterPageQuery {
                first: INVENTORY - 4,
                revision,
                conversion: None,
            },
        )
    };
    let last = last_page(&mut instance, first.revision);
    assert_eq!(
        last.as_slice()
            .iter()
            .map(|parameter| parameter.id)
            .collect::<Vec<_>>(),
        [3590, 3591, 3592, 7]
    );
    let mut input = request(instance.configuration(), 0);
    input
        .parameters
        .push(crate::transport::ParameterPoint {
            frame: 0,
            id: 7,
            normalized: 0.5,
        })
        .unwrap();
    let output = instance.exchange_on_worker(input).unwrap();
    assert_eq!(&output.pcm[..8], &input.pcm[..8]);
    let updated = page(&mut instance, ParameterPageQuery::default());
    let updated_gain = last_page(&mut instance, updated.revision);
    assert_eq!(
        (updated.total, updated_gain.as_slice()[3].normalized),
        (INVENTORY, 0.5)
    );
    let stopped = instance.quiesce_retained_on_control().unwrap();
    let (stopped, captured) = stopped.capture_state_retained_on_control().unwrap();
    assert!(stopped.retire_on_control().unwrap().status.success());
    let independent = Case::new();
    let mut restored = start(&independent, "2593", Some(captured)).unwrap();
    let values = page(&mut restored, ParameterPageQuery::default());
    let restored_gain = last_page(&mut restored, values.revision);
    assert_eq!(
        (values.total, restored_gain.as_slice()[3].normalized),
        (INVENTORY, 0.5)
    );
    assert!(
        restored
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
    for (malformed, diagnostic) in [
        ("2625", "inventory exceeds the host capacity"),
        ("duplicate", "invalid controller parameter inventory"),
    ] {
        let case = Case::new();
        let error = start(&case, malformed, None)
            .err()
            .expect("inventory must refuse");
        assert!(error.to_string().contains(diagnostic), "{error}");
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
                ParameterPageProgress::Page(_)
                | ParameterPageProgress::Conversion(_)
                | ParameterPageProgress::Editor(_) => {
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
                ParameterPageProgress::Conversion(_) | ParameterPageProgress::Editor(_) => {
                    panic!("expected stopped metadata page")
                }
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

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn authored_parameter_metadata_uses_loaded_class_and_sdk_flags() {
    let build = build();
    for (variant, id, steps) in [
        (80, 0, Some(0)),
        (81, 7, Some(3)),
        (83, 7, None),
        (84, 7, None),
    ] {
        let case = Case::new();
        let setup = case.setup(variant);
        let mut instance = Prepared::prepare(
            &build,
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            move |command| {
                setup(command);
                command.env("HEX_ORIGINAL_PARAMETER_INVENTORY", "9");
            },
        )
        .unwrap();
        let binding = instance.configuration().binding;
        let class = instance.configuration().identity.class;
        let wrong_class = ClassId::new([0xff; 16]).unwrap();
        assert!(
            instance
                .read_authored_parameter_metadata_on_control(wrong_class, &[id])
                .is_err()
        );
        assert!(
            instance
                .read_authored_parameter_metadata_on_control(class, &[id, 99])
                .is_err()
        );
        assert!(
            instance
                .read_authored_parameter_metadata_on_control(class, &[1001])
                .is_err()
        );
        let result = instance.read_authored_parameter_metadata_on_control(class, &[id]);
        if let Some(steps) = steps {
            let facts = result.unwrap();
            assert_eq!(facts.class(), class);
            assert_eq!(facts.binding(), binding);
            assert!(facts.revision() > 0);
            assert_eq!(facts.step_count(id), Some(steps));
        } else {
            assert!(result.is_err());
        }
        assert_eq!(instance.completed_blocks_on_control(), 0);
        assert!(
            instance
                .stop_and_reap_on_control()
                .unwrap()
                .status
                .success()
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn authored_pcm_rechecks_processing_capability_after_controller_refresh() {
    use crate::transport::{ParameterPoint, ProcessTiming};

    let build = build();
    for variant in [141, 142] {
        let case = Case::new();
        let prepare = |case: &Case| {
            Prepared::prepare(
                &build,
                source(),
                options(),
                PreparationCancellation::new_on_control().unwrap(),
                case.setup(variant),
            )
            .unwrap()
        };
        let mut instance = prepare(&case);
        let class = instance.configuration().identity.class;
        let metadata = instance
            .read_authored_parameter_metadata_on_control(class, &[7])
            .unwrap();
        assert_eq!(metadata.step_count(7), Some(0));
        let input = |instance: &Prepared, sequence| {
            let mut block = request(instance.configuration(), sequence);
            block.timing = Some(ProcessTiming {
                playing: true,
                arrangement_frame: block.first_frame as i64,
                arrangement_beat_raw: None,
                tempo_micro_bpm: None,
                meter: Some((4, 4)),
                cycle_beats_raw: None,
            });
            for (frame, normalized) in [(0, 0.25), (7, 0.75)] {
                block
                    .parameters
                    .push(ParameterPoint {
                        frame,
                        id: 7,
                        normalized,
                    })
                    .unwrap();
            }
            block
        };

        let other_case = Case::new();
        let mut other = prepare(&other_case);
        assert_eq!(
            other.configuration().binding,
            instance.configuration().binding
        );
        assert!(matches!(
            other.exchange_authored_on_worker(input(&other, 0), &metadata),
            Err(Error::Protocol("authored parameter source differs"))
        ));
        assert_eq!(other.completed_blocks_on_control(), 0);
        assert!(other.stop_and_reap_on_control().unwrap().status.success());

        for sequence in 0..2 {
            let block = input(&instance, sequence);
            let reply = instance
                .exchange_authored_on_worker(block, &metadata)
                .unwrap();
            for (frame, (input, output)) in block.pcm[..8].iter().zip(&reply.pcm[..8]).enumerate() {
                let normalized = 0.25 + 0.5 * frame as f64 / 7.0;
                let gain = (normalized * 4.0 * (0.5 + sequence as f64 * 0.125)) as f32;
                for channel in 0..2 {
                    assert!((output[channel] - input[channel] * gain).abs() < 1e-6);
                }
            }
            let current = page(&mut instance, ParameterPageQuery::default());
            assert!(current.revision > metadata.revision());
        }

        fs::write(case.release(), b"change authored parameter capability").unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let current = page(&mut instance, ParameterPageQuery::default());
            let gain = &current.as_slice()[0];
            if gain.step_count != 0 || !gain.supports_authored_automation() {
                break;
            }
            assert!(Instant::now() < until, "SDK metadata did not refresh");
            thread::yield_now();
        }
        assert!(matches!(
            instance.exchange_authored_on_worker(input(&instance, 2), &metadata),
            Err(Error::Worker(8))
        ));
        assert_eq!(instance.completed_blocks_on_control(), 2);
        assert!(
            !instance
                .cancel_and_reap_on_control()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(
            case.observation().unwrap()[10],
            2,
            "stale input entered SDK DSP"
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn authored_quantum_partition_preserves_signed_pcm_after_pipeline_latency() {
    use crate::transport::{
        AuthoredParameterProgram, AuthoredSegment, Fault, ParameterPoint, Parameters,
        ProcessTiming, SegmentTiming, TimingChanges, channel,
    };
    use std::num::NonZeroU64;

    struct FixtureEnvelope;
    impl FixtureEnvelope {
        fn normalized(frame: u64) -> f64 {
            if frame < 8 {
                0.25 + frame as f64 / 16.0
            } else {
                0.75
            }
        }
    }
    impl AuthoredParameterProgram for FixtureEnvelope {
        fn quantum_program_key(&self) -> NonZeroU64 {
            NonZeroU64::MIN
        }

        fn render_quantum(
            &self,
            origins: &[Option<u64>],
            _: &TimingChanges,
            output: &mut Parameters,
        ) -> std::result::Result<(), Fault> {
            for (frame, &origin) in origins.iter().enumerate() {
                if let Some(origin) = origin {
                    output.push(ParameterPoint {
                        frame: frame as u16,
                        id: 0,
                        normalized: Self::normalized(origin),
                    })?;
                }
            }
            Ok(())
        }
    }

    let build = build();
    let origins = [
        Some(0),
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(5),
        Some(6),
        Some(7),
        None,
        None,
        None,
        None,
        Some(20),
        Some(21),
        Some(0),
        Some(1),
    ];
    for partitions in [[8, 8, 8, 8, 0], [3, 6, 8, 7, 8]] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build,
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(80),
        )
        .unwrap();
        let class = instance.configuration().identity.class;
        let metadata = instance
            .read_authored_parameter_metadata_on_control(class, &[0])
            .unwrap();
        let (_control, mut callback, mut worker) = channel(metadata.binding(), 8).unwrap();
        instance
            .arm_authored_parameter_metadata_on_control(metadata)
            .unwrap();
        let mut position = 0;
        let mut pcm = [[0.0; 2]; 32];
        for frames in partitions.into_iter().filter(|&frames| frames != 0) {
            callback.begin_physical_callback(frames).unwrap();
            let cut = frames / 2;
            for (offset, len) in [(0, cut), (cut, frames - cut)] {
                let origin = |index| origins[(position + offset + index) % origins.len()];
                callback
                    .stage_segment_with_authored_parameters(
                        offset,
                        &[[0.25, -0.125]; 8][..len],
                        &[],
                        &mut pcm[position + offset..position + offset + len],
                        SegmentTiming {
                            playing: None,
                            at: |index| {
                                Some(ProcessTiming {
                                    arrangement_frame: origin(index)? as i64,
                                    playing: true,
                                    arrangement_beat_raw: None,
                                    tempo_micro_bpm: None,
                                    meter: None,
                                    cycle_beats_raw: None,
                                })
                            },
                        },
                        AuthoredSegment {
                            program: &FixtureEnvelope,
                            origin: &origin,
                        },
                    )
                    .unwrap();
            }
            assert!(worker.receive_request().unwrap().is_none());
            callback.finish(true).unwrap();
            if let Some(request) = worker.receive_request().unwrap() {
                assert!(request.authored_program.is_some());
                worker
                    .complete(
                        instance
                            .exchange_armed_authored_on_worker(request.block)
                            .unwrap(),
                    )
                    .unwrap();
            }
            position += frames;
        }
        assert_eq!(position, pcm.len());
        assert!(pcm[..16].iter().all(|frame| *frame == [0.0; 2]));
        for (index, (&origin, actual)) in origins.iter().zip(&pcm[16..]).enumerate() {
            // Compensation had no origin for frames 8–11. The first SDK
            // slice holds the previous quantum's last value, even though a
            // new authored value arrives at offset four of the same request.
            let sdk_slice = if index < 8 {
                0
            } else if index < 12 {
                1
            } else {
                2
            };
            let gain = (FixtureEnvelope::normalized(origin.unwrap_or(7))
                * 4.0
                * (0.5 + sdk_slice as f64 * 0.125)) as f32;
            assert_eq!(*actual, [0.25 * gain, -0.125 * gain], "frame {index}");
        }
        assert!(
            instance
                .stop_and_reap_on_control()
                .unwrap()
                .status
                .success()
        );
    }
}
