//! Original SDK reader capability extension, independent of host graph admission.
use super::*;
use crate::native::AuthoredParameterExtensionProgress as Progress;

fn poll_extension(instance: &mut Prepared) -> Progress {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let progress = instance
            .poll_authored_parameter_extension_on_worker()
            .unwrap();
        if progress != Progress::Pending {
            return progress;
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}

fn query(observed: &ParameterPage) -> ParameterPageQuery {
    ParameterPageQuery {
        first: observed.first,
        revision: observed.revision,
        conversion: None,
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn sdk_extension_preserves_existing_bases_and_backing_while_pcm_continues() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(144),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    let before = page(&mut instance, ParameterPageQuery::default());
    assert!(
        instance
            .begin_authored_parameter_extension_on_worker(class(), 11, 0, query(&before))
            .is_err()
    );
    assert!(
        instance
            .reserve_authored_parameter_capacity_on_control(0)
            .is_err()
    );
    let unreserved = instance.retained_storage_on_control().unwrap();
    let mut quoted = None;
    let measured = allocation_counter::measure(|| {
        quoted = instance.storage_with_authored_capacity_on_control(2)
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    instance
        .reserve_authored_parameter_capacity_on_control(2)
        .unwrap();
    let reserved = instance.retained_storage_on_control().unwrap();
    assert_eq!(quoted, Some(reserved));
    assert_eq!(
        instance.storage_with_authored_capacity_on_control(2),
        Some(reserved)
    );
    assert!(
        instance
            .storage_with_authored_capacity_on_control(0)
            .is_none()
    );
    let metadata_bytes = instance
        .authored_parameter_metadata
        .as_ref()
        .unwrap()
        .backing_data_bytes()
        .unwrap();
    assert_eq!(reserved.data_bytes, unreserved.data_bytes + metadata_bytes);
    assert_eq!(
        reserved.shared_handle_bound,
        unreserved.shared_handle_bound + 1
    );
    let metadata = instance
        .read_authored_parameter_metadata_on_control(class(), &[7])
        .unwrap();
    assert_eq!(
        metadata.initial_normalized(7),
        Some(0.25),
        "an empty reservation must preserve the first canonical observation"
    );
    instance
        .arm_authored_parameter_metadata_on_control(metadata)
        .unwrap();
    let storage = instance.retained_storage_on_control().unwrap();
    let observed = page(&mut instance, ParameterPageQuery::default());
    let q = query(&observed);
    assert!(
        instance
            .begin_authored_parameter_extension_on_worker(ClassId::new([9; 16]).unwrap(), 11, 0, q)
            .is_err()
    );
    instance
        .begin_authored_parameter_extension_on_worker(class(), 99, 0, q)
        .unwrap();
    assert!(matches!(
        poll_extension(&mut instance),
        Progress::Refused(_)
    ));
    assert!(!instance.has_armed_authored_parameter(11));
    instance
        .begin_authored_parameter_extension_on_worker(class(), 11, 1, q)
        .unwrap();
    assert!(matches!(
        poll_extension(&mut instance),
        Progress::Refused(_)
    ));
    assert!(instance.has_armed_authored_parameter(7));

    instance
        .begin_authored_parameter_extension_on_worker(class(), 11, 0, q)
        .unwrap();
    assert!(instance.begin_parameter_page_on_worker(q).is_err());
    assert!(
        instance
            .begin_authored_parameter_extension_on_worker(class(), 11, 0, q)
            .is_err()
    );
    assert!(
        instance.poll_parameter_page_on_worker().is_err(),
        "ordinary observation cannot consume an extension terminal"
    );
    assert!(instance.stop_and_reap_on_control().is_err());
    let input = request(instance.configuration(), 0);
    let output = instance.exchange_armed_authored_on_worker(input).unwrap();
    assert_eq!(output.pcm[..8], input.pcm[..8]);
    assert!(matches!(
        poll_extension(&mut instance),
        Progress::Armed {
            id: 11,
            steps: 0,
            ..
        }
    ));
    assert!(instance.has_armed_authored_parameter(11));
    let metadata = instance.authored_parameter_metadata.as_ref().unwrap();
    assert_eq!(metadata.initial_normalized(7), Some(0.25));
    assert_eq!(
        metadata.initial_normalized(11),
        None,
        "current controller values are not canonical base witnesses"
    );
    let mut input = request(instance.configuration(), 1);
    input.timing = Some(crate::transport::ProcessTiming {
        playing: true,
        arrangement_frame: input.first_frame as i64,
        arrangement_beat_raw: None,
        tempo_micro_bpm: None,
        meter: Some((4, 4)),
        cycle_beats_raw: None,
    });
    input
        .parameters
        .push(crate::transport::ParameterPoint {
            frame: 0,
            id: 11,
            normalized: 0.5,
        })
        .unwrap();
    let output = instance.exchange_armed_authored_on_worker(input).unwrap();
    for (output, input) in output.pcm[..8].iter().zip(&input.pcm[..8]) {
        assert_eq!(
            *output,
            [input[0] * 2.0, input[1] * 2.0],
            "the newly armed target must reach actual SDK processing"
        );
    }
    assert_eq!(instance.retained_storage_on_control().unwrap(), storage);
    assert!(
        instance
            .reserve_authored_parameter_capacity_on_control(3)
            .is_err()
    );
    assert!(
        instance
            .begin_authored_parameter_extension_on_worker(class(), 99, 0, q)
            .is_err(),
        "full metadata refuses before admission"
    );
    let refreshed = page(&mut instance, ParameterPageQuery::default());
    instance
        .begin_authored_parameter_extension_on_worker(class(), 11, 0, query(&refreshed))
        .unwrap();
    assert!(matches!(
        poll_extension(&mut instance),
        Progress::Armed { id: 11, .. }
    ));
    let changed = ParameterPageQuery {
        revision: refreshed.revision.checked_add(1).unwrap(),
        ..query(&refreshed)
    };
    instance
        .begin_authored_parameter_extension_on_worker(class(), 11, 0, changed)
        .unwrap();
    assert!(matches!(
        poll_extension(&mut instance),
        Progress::Changed { .. }
    ));
    assert_eq!(instance.retained_storage_on_control().unwrap(), storage);
    assert_eq!(instance.child_id_on_control(), child);
    let old = instance.configuration().binding;
    let stopped = instance.quiesce_retained_on_control().unwrap();
    instance = stopped
        .resume_retained_on_control(
            Binding {
                epoch: old.epoch + 1,
                ..old
            },
            Some(options().presentation),
        )
        .unwrap();
    assert!(!instance.has_armed_authored_parameter(11));
    assert!(
        instance
            .begin_authored_parameter_extension_on_worker(class(), 11, 0, q)
            .is_err(),
        "an old epoch cannot grant capability to its successor"
    );
    let quoted = instance
        .storage_with_authored_capacity_on_control(2)
        .unwrap();
    instance
        .reserve_authored_parameter_capacity_on_control(2)
        .unwrap();
    assert_eq!(instance.retained_storage_on_control(), Some(quoted));
    assert!(!instance.has_armed_authored_parameter(7));
    let refreshed = page(&mut instance, ParameterPageQuery::default());
    instance
        .begin_authored_parameter_extension_on_worker(class(), 11, 0, query(&refreshed))
        .unwrap();
    assert!(matches!(
        poll_extension(&mut instance),
        Progress::Armed { id: 11, .. }
    ));
    assert!(!instance.has_armed_authored_parameter(7));
    let renewed = instance
        .read_authored_parameter_metadata_on_control(class(), &[7, 11])
        .unwrap();
    assert_eq!(renewed.initial_normalized(7), Some(0.25));
    assert_eq!(renewed.initial_normalized(11), None);
    instance
        .arm_authored_parameter_metadata_on_control(renewed)
        .unwrap();
    assert!(instance.has_armed_authored_parameter(11));
    assert!(
        instance
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(case.observation().unwrap()[1], 0);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn held_extension_keeps_pcm_running_and_cancellation_reaps_the_original_reader() {
    let case = Case::new();
    let cancellation = PreparationCancellation::new_on_control().unwrap();
    let token = cancellation.token_on_control();
    let mut instance =
        Prepared::prepare(&build(), source(), options(), cancellation, case.setup(93)).unwrap();
    instance
        .reserve_authored_parameter_capacity_on_control(1)
        .unwrap();
    // A stale revision still performs the real SDK enumeration before it can
    // report Changed. The fixture holds that actual main-lane formatter.
    instance
        .begin_authored_parameter_extension_on_worker(
            class(),
            7,
            0,
            ParameterPageQuery {
                revision: 1,
                ..Default::default()
            },
        )
        .unwrap();
    wait_controller_held(&case);
    assert_eq!(
        instance
            .poll_authored_parameter_extension_on_worker()
            .unwrap(),
        Progress::Pending
    );
    assert!(instance.stop_and_reap_on_control().is_err());
    let input = request(instance.configuration(), 0);
    instance.exchange_on_worker(input).unwrap();
    assert_eq!(instance.completed_blocks_on_control(), 1);
    assert!(!instance.has_armed_authored_parameter(7));
    token.cancel_on_control();
    assert!(matches!(
        instance.poll_authored_parameter_extension_on_worker(),
        Err(Error::Cancelled)
    ));
    assert!(
        instance
            .cancel_and_reap_on_control()
            .unwrap()
            .terminal
            .is_none()
    );
    assert!(instance.child_id_on_control().is_none());
    assert!(!case.release().exists());
    assert_eq!(case.observation().unwrap()[1], 0);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn sdk_extension_checks_raw_zero_flags_and_original_source_cancellation() {
    for (variant, id, steps, eligible) in [(80, 0, 0, true), (81, 7, 3, true), (83, 7, 0, false)] {
        let case = Case::new();
        let cancellation = PreparationCancellation::new_on_control().unwrap();
        let token = cancellation.token_on_control();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            cancellation,
            case.setup(variant),
        )
        .unwrap();
        instance
            .reserve_authored_parameter_capacity_on_control(1)
            .unwrap();
        let observed = page(&mut instance, ParameterPageQuery::default());
        let storage = instance.retained_storage_on_control().unwrap();
        instance
            .begin_authored_parameter_extension_on_worker(class(), id, steps, query(&observed))
            .unwrap();
        let result = poll_extension(&mut instance);
        assert_eq!(matches!(result, Progress::Armed { .. }), eligible);
        assert_eq!(instance.has_armed_authored_parameter(id), eligible);
        assert_eq!(instance.retained_storage_on_control().unwrap(), storage);
        instance
            .begin_authored_parameter_extension_on_worker(class(), id, steps, query(&observed))
            .unwrap();
        token.cancel_on_control();
        assert!(matches!(
            instance.poll_authored_parameter_extension_on_worker(),
            Err(Error::Cancelled)
        ));
        assert!(
            instance
                .cancel_and_reap_on_control()
                .unwrap()
                .terminal
                .is_none()
        );
        assert!(instance.child_id_on_control().is_none());
    }
}
