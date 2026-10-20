use super::*;
fn construct(reservation: Option<AudioCandidatePublicationReservation>) -> Result<Queues, Invalid> {
    super::construct(reservation, 48000)
}
fn target_ring_bytes() -> usize {
    spectrum_target::constructor_cost().unwrap().0
        + processor_audition::constructor_cost().unwrap().0
        + crate::sample_preview::constructor_cost().unwrap().0
        + crate::gain_recording::constructor_cost().unwrap().0
        - 8 * shared_atomic_bytes().unwrap()
}
fn target_existing_ring_bytes(queues: &Queues) -> usize {
    queues
        .spectrum
        .1
        .constructor_channel_storage()
        .unwrap()
        .data_bytes
        + queues
            .audition
            .1
            .constructor_channel_storage()
            .unwrap()
            .data_bytes
        + queues
            .preview
            .1
            .constructor_channel_storage()
            .unwrap()
            .data_bytes
        + queues
            .gain
            .1
            .constructor_channel_storage()
            .unwrap()
            .data_bytes
        - 8 * size_of::<AtomicU64>()
}
fn reservation(retained: usize, peak: usize) -> AudioCandidatePublicationReservation {
    AudioCandidatePublicationReservation {
        runtime_slots: 3,
        archive_slots: 3,
        member_rows: 128,
        transfer_rows: 128,
        receipt_rows: 3,
        data_rows: 256,
        handle_rows: 256,
        max_retained_data_bytes: retained,
        max_construction_peak_data_bytes: peak,
    }
}
#[test]
fn actual_root_nine_queues_status_exact_and_each_minus_one_before_all_births() {
    let (retained, peak) = native_birth_bound().unwrap();
    let (old_retained, old_peak) = root_queue_bound().unwrap();
    for limits in [
        reservation(retained - 1, peak),
        reservation(retained, peak - 1),
        // The formerly accepted Root/three-queue-only allowance must no longer
        // start any constructor and allocate Status outside that allowance.
        reservation(old_retained, old_peak),
    ] {
        let measured = allocation_counter::measure(|| {
            assert!(matches!(
                construct(Some(limits)),
                Err(Invalid(
                    "prepared Root/queue/status birth reservation exceeded"
                ))
            ));
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    let mut actual = None;
    let measured = allocation_counter::measure(|| {
        actual = Some(construct(Some(reservation(retained, peak))).unwrap())
    });
    assert_eq!(
        (measured.count_total, measured.bytes_total),
        (45, bound().unwrap().0 as u64)
    );
    let Queues {
        status,
        spectrum,
        audition,
        preview,
        gain,
        root,
        publications,
        incoming,
        returning,
        retired,
        mut commands,
        mut events,
    } = actual.unwrap();
    let actual_status = status_storage::capture_callback_status_for_factory(&status.publication);
    assert_eq!(
        actual_status,
        Some(status_storage::fresh_requested_bytes().unwrap())
    );
    let status_bytes = actual_status.unwrap();
    assert_eq!(
        root.test_atomic_signal_data().unwrap(),
        (8, 8 * shared_atomic_bytes().unwrap())
    );
    assert_eq!(
        root.test_status_data().unwrap(),
        status_owner::StatusArchive::requested_bytes().unwrap()
    );
    let status_drop = allocation_counter::measure(|| drop(status));
    assert_eq!(
        (status_drop.count_current, status_drop.bytes_current),
        (0, 0)
    );
    let rings = ring_bytes::<QueuedPublication>(1).unwrap()
        + ring_bytes::<ReturnedPublication>(RETIRED).unwrap()
        + ring_bytes::<QueuedCommand>(COMMANDS).unwrap()
        + target_ring_bytes();
    assert_eq!(root.test_prepared_ring_data().unwrap(), (9, rings));
    let transfers = allocation_counter::measure(|| {
        // The actual typed queue is used, not a surrogate test transport.
        assert!(commands.write_chunk_uninit(0).is_ok());
        assert!(events.pop().is_err());
    });
    assert_eq!((transfers.count_total, transfers.count_current), (0, 0));
    drop(publications);
    drop(incoming);
    drop(returning);
    drop(retired);
    drop(commands);
    drop(events);
    drop(spectrum);
    drop(audition);
    drop(preview);
    drop(gain);
    // Unlike the old raw Atomic Arc handles, the actual immutable anchors
    // remain in Root and are still physically allocated after both field drops.
    assert_eq!(
        root.test_atomic_signal_data().unwrap(),
        (8, 8 * shared_atomic_bytes().unwrap())
    );
    assert_eq!(
        root.test_prepared_ring_data().unwrap(),
        (
            9,
            9 * NativeFixedRingRegistration::metadata_requested_bytes_on_control().unwrap()
        )
    );
    eprintln!(
        "SUPPORTED real queue factory45births with future4carrier reservation retained{retained}/peak{peak}; actual queue allocation excludes not-yet-born carriers; Status{status_bytes} actualDATAbytes/normalfielddrops0free whileactualRootanchorretainsDATA; retained/peak-minus1 before any allocation. Actual Root retains9 ring records and8 realScalar anchors with individual field lineage through normal endpoint drops. Status/OTHERchannel/family/native inputs remain mandatory; not full factory admission"
    );
}
#[test]
fn actual_resident_legacy_queues_are_not_adopted_by_new_factory_record_path() {
    let queues = construct(None).unwrap();
    assert_eq!(queues.root.test_prepared_ring_data().unwrap(), (0, 0));
    assert!(CandidateRoot::requested_birth_bytes(usize::MAX).is_none());
    eprintln!(
        "SUPPORTED legacy/resident constructor records stay absent/unqualified; no retrospective adoption, Root slot layout overflow refuses"
    );
}

#[test]
fn actual_paired_spectrum_and_audition_commands_keep_bounded_zero_alloc_transitions() {
    let (retained, peak) = native_birth_bound().unwrap();
    let mut queues = construct(Some(reservation(retained, peak))).unwrap();
    let before = queues.root.test_prepared_ring_data().unwrap();
    let measured = allocation_counter::measure(|| {
        for id in 1..=64 {
            let identity = hex_engine::PostNodeIdentity::new(
                hex_engine::ProcessorChainOwner::Track(1),
                id,
                hex_engine::DeviceTypeId::from_static("hex.eq"),
            );
            let selected = queues.spectrum.0.select(identity, 2, 1, 48000).unwrap().0;
            queues.spectrum.1.begin_block();
            assert_eq!(queues.spectrum.1.current(), Some(selected));
            let audition = queues
                .audition
                .0
                .select(
                    identity,
                    2,
                    1,
                    48000,
                    hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 },
                )
                .unwrap()
                .0;
            queues.audition.1.begin_block();
            assert_eq!(queues.audition.1.current(), Some(audition));
            queues.spectrum.0.clear().unwrap();
            queues.audition.0.cancel_immediately().unwrap();
            queues.spectrum.1.begin_block();
            queues.audition.1.begin_block();
            assert_eq!(queues.spectrum.1.current(), None);
            assert_eq!(queues.audition.1.current(), None);
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_total,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(queues.root.test_prepared_ring_data().unwrap(), before);
    eprintln!(
        "SUPPORTED actual constructor-paired target/audition queues64 select/clear/cancel rounds0alloc/free, identities preserved and all9Root records unchanged; no analyzer/DSP/OS acceptance from command test"
    );
}

#[test]
fn actual_paired_preview_pcm_and_gain_written_receipts_preserve_callback_zero_alloc_free() {
    let (retained, peak) = native_birth_bound().unwrap();
    let mut queues = construct(Some(reservation(retained, peak))).unwrap();
    let pcm = hex_engine::Audio::new(48000, vec![[0.25, -0.5]; 4]).unwrap();
    let generation = queues.preview.0.request().unwrap();
    let target = crate::gain_recording::Target {
        track_id: 1,
        project_generation: 2,
        canonical_revision: 3,
        audio_revision: 4,
        recording_epoch: 1,
        transport_epoch: 1,
        gesture_serial: 1,
        sample_rate: 48000,
    };
    let measured = allocation_counter::measure(|| {
        queues.preview.0.publish(generation, pcm).unwrap();
        queues.preview.1.begin_block(false);
        let mut output = [[0.0; 2]; 4];
        queues.preview.1.mix(&mut output);
        assert_eq!(output, [[0.25, -0.5]; 4]);
        queues.preview.1.begin_block(false); // actual completed PCM moves to control retirement queue
        assert!(queues.preview.1.is_empty());
        assert_eq!(
            queues.preview.0.snapshot().phase,
            crate::sample_preview::Phase::Finished
        );

        queues.gain.0.begin(target, 0.25).unwrap();
        assert_eq!(
            queues.gain.1.prepare(|_| Ok(crate::gain_recording::Clock {
                performance_frame: 0,
                content_frame: 100,
                delivery_delay: 0,
                loop_pass: 0,
            })),
            Some((1, 0.25))
        );
        assert!(queues.gain.0.try_receipt().is_none());
        queues.gain.1.finish_quantum(8, true);
        assert!(
            matches!(queues.gain.0.try_receipt(), Some(crate::gain_recording::Receipt::Applied {
            target: actual, gain: 0.25, performance_frame: 0, content_frame: 100, ..
        }) if actual == target)
        );
        queues.gain.0.release().unwrap();
        assert_eq!(
            queues.gain.1.prepare(|_| Ok(crate::gain_recording::Clock {
                performance_frame: 8,
                content_frame: 108,
                delivery_delay: 0,
                loop_pass: 0,
            })),
            None
        );
        queues.gain.1.finish_quantum(16, true);
        assert!(
            matches!(queues.gain.0.try_receipt(), Some(crate::gain_recording::Receipt::Finished {
            target: actual, performance_frame: 8, content_frame: 108, ..
        }) if actual == target)
        );
        assert!(queues.gain.0.target().is_none());
    });
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_total,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let actual_pcm_drop = allocation_counter::measure(|| queues.preview.0.reclaim());
    assert!(actual_pcm_drop.count_current < 0 && actual_pcm_drop.bytes_current < 0);
    assert_eq!(queues.root.test_prepared_ring_data().unwrap().0, 9);
    eprintln!(
        "SUPPORTED constructor-paired preview actual4frame stereoPCM plus gain actualwritten Applied/Finished ACKs0alloc/free; PCM reclaimed ONLYcontrol, all9Root ring records preserved. PreviewPCM lineage/fullSampler/native admission NOT inferred"
    );
}

#[test]
fn actual_domain_configuration_retains_nine_records_and_charges_only_old_budget_omissions() {
    let (retained, peak) = native_birth_bound().unwrap();
    let queues = construct(Some(reservation(retained, peak))).unwrap();
    let old_queue_bodies = size_of::<rtrb::RingBuffer<QueuedPublication>>()
        + size_of::<QueuedPublication>()
        + size_of::<rtrb::RingBuffer<ReturnedPublication>>()
        + RETIRED * size_of::<ReturnedPublication>()
        + size_of::<rtrb::RingBuffer<QueuedCommand>>()
        + COMMANDS * size_of::<QueuedCommand>()
        + target_existing_ring_bytes(&queues);
    let ring_bytes = ring_bytes::<QueuedPublication>(1).unwrap()
        + ring_bytes::<ReturnedPublication>(RETIRED).unwrap()
        + ring_bytes::<QueuedCommand>(COMMANDS).unwrap()
        + target_ring_bytes();
    let required = CandidateRoot::requested_birth_bytes(RETIRED).unwrap() + ring_bytes
        - old_queue_bodies
        + 8 * (shared_atomic_bytes().unwrap() - size_of::<AtomicU64>())
        + hex_engine::sampler_host_fields::NativeStatusRegistration::requested_bytes_on_control()
            .unwrap()
        + CandidateRoot::resident_requested_bytes().unwrap()
        // Separate registration allocation, not the handle already inline in
        // requested_birth_bytes. The old native queue report never contained it.
        + CandidateRoot::root_fields_requested_bytes().unwrap();
    let short = allocation_counter::measure(|| {
        assert!(
            queues
                .root
                .configure_domains(crate::AudioCandidateDomainReservation {
                    max_retained_data_bytes: required - 1,
                    max_construction_peak_data_bytes: required,
                })
                .is_err()
        );
    });
    assert_eq!((short.count_total, short.count_current), (0, 0));
    assert_eq!(
        queues.root.test_prepared_ring_data().unwrap(),
        (9, ring_bytes)
    );
    let exact = allocation_counter::measure(|| {
        queues
            .root
            .configure_domains(crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: required,
                max_construction_peak_data_bytes: required,
            })
            .unwrap();
    });
    assert_eq!(
        (
            exact.count_total,
            exact.count_current,
            exact.bytes_total,
            exact.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        queues.root.test_prepared_ring_data().unwrap(),
        (9, ring_bytes)
    );
    eprintln!(
        "SUPPORTED actual9 records survive zeroalloc configure; old core/backing budget not charged twice, actual omitted core-header/padding+9metadata is charged before configure accepts. Domain-only test is NOT full native/channel admission"
    );
}

#[test]
fn actual_registered_constructor_channel_preserves_silence_and_callback_zero_alloc_free() {
    let (_, peak) = native_birth_bound().unwrap();
    let (control, mut callback) = channel_with_factory_reservation(
        48000,
        None,
        Some(reservation(1 << 20, (2 << 20).max(peak))),
    )
    .unwrap();
    let root = control.native_candidate_root();
    assert_eq!(root.test_prepared_ring_data().unwrap().0, 9);
    let mut output = [[1.0_f32; 2]; 128];
    let measured = allocation_counter::measure(|| {
        for _ in 0..64 {
            callback.render(&mut output).unwrap();
            assert!(output.iter().all(|frame| *frame == [0.0; 2]));
        }
        control.stop();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_total,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    drop(callback);
    drop(control);
    assert_eq!(
        root.test_prepared_ring_data().unwrap(),
        (
            9,
            9 * NativeFixedRingRegistration::metadata_requested_bytes_on_control().unwrap()
        )
    );
    eprintln!(
        "SUPPORTED real registered channel unchanged silent128frame/64turn+Stop callback0alloc/free; actual endpoint normaldisposal leaves only Root-heldmetadata. No nativeSome/activeSampler/fullstartup qualification from silence"
    );
}

#[test]
fn actual_invalid_publication_shape_refuses_before_any_root_queue_birth() {
    let (retained, peak) = native_birth_bound().unwrap();
    let valid = reservation(retained, peak);
    for invalid in [
        AudioCandidatePublicationReservation {
            runtime_slots: 0,
            ..valid
        },
        AudioCandidatePublicationReservation {
            archive_slots: 0,
            ..valid
        },
        AudioCandidatePublicationReservation {
            receipt_rows: 0,
            ..valid
        },
        AudioCandidatePublicationReservation {
            max_retained_data_bytes: peak + 1,
            ..valid
        },
    ] {
        let measured = allocation_counter::measure(|| {
            assert!(matches!(
                construct(Some(invalid)),
                Err(Invalid("invalid constructor publication reservation"))
            ));
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    eprintln!(
        "SUPPORTED same canonical publication shape validation now precedes first Root/queue birth as well as later publication bank preparation;4invalid shapes0alloc/free"
    );
}

#[test]
fn actual_later_publication_budget_equals_real_existing_root_queues_plus_new_cell_births() {
    let limits = reservation(1 << 20, 2 << 20);
    let root_bytes = bound().unwrap().0;
    let first = construct(Some(limits)).unwrap();
    let old_queue_bodies = size_of::<rtrb::RingBuffer<QueuedPublication>>()
        + size_of::<QueuedPublication>()
        + size_of::<rtrb::RingBuffer<ReturnedPublication>>()
        + RETIRED * size_of::<ReturnedPublication>()
        + size_of::<rtrb::RingBuffer<QueuedCommand>>()
        + COMMANDS * size_of::<QueuedCommand>()
        + target_existing_ring_bytes(&first)
        + 8 * size_of::<AtomicU64>()
        + status_storage::fresh_requested_bytes().unwrap();
    let allocated = allocation_counter::measure(|| {
        first
            .root
            .configure_publications(limits, old_queue_bodies)
            .unwrap()
    });
    let actual_total = root_bytes + allocated.bytes_total as usize;
    assert_eq!(
        first.root.domain_usage().unwrap().retained_data_reservation,
        actual_total,
        "old core/backing DATA counted once, actual header/padding/registration and new cell backings all present"
    );
    for cap in [actual_total - 1, actual_total] {
        let next = construct(Some(limits)).unwrap();
        let mut selected = limits;
        selected.max_retained_data_bytes = cap;
        selected.max_construction_peak_data_bytes = actual_total;
        let measured = allocation_counter::measure(|| {
            let result = next.root.configure_publications(selected, old_queue_bodies);
            assert_eq!(result.is_ok(), cap == actual_total);
        });
        if cap < actual_total {
            assert_eq!(
                (
                    measured.count_total,
                    measured.count_current,
                    measured.bytes_total,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        } else {
            assert_eq!(root_bytes + measured.bytes_total as usize, actual_total);
        }
        assert_eq!(next.root.test_prepared_ring_data().unwrap().0, 9);
    }
    eprintln!(
        "SUPPORTED later publication retained accounting EXACT{actual_total}=actual first Root+9queues plus allocator-measured new cell/receipt/Life births; minus1 refuses before new cell birth; records persist; not full native/host lifetime admission"
    );
}

#[test]
fn actual_status_role_metadata_is_in_later_union_before_new_cell_births() {
    let limits = reservation(1 << 20, 2 << 20);
    let existing = construct(Some(limits)).unwrap();
    let old_reported_children = size_of::<rtrb::RingBuffer<QueuedPublication>>()
        + size_of::<QueuedPublication>()
        + size_of::<rtrb::RingBuffer<ReturnedPublication>>()
        + RETIRED * size_of::<ReturnedPublication>()
        + size_of::<rtrb::RingBuffer<QueuedCommand>>()
        + COMMANDS * size_of::<QueuedCommand>()
        + target_existing_ring_bytes(&existing)
        + 8 * size_of::<AtomicU64>()
        + status_storage::fresh_requested_bytes().unwrap();
    let measured_new = allocation_counter::measure(|| {
        existing
            .root
            .configure_publications(limits, old_reported_children)
            .unwrap();
    });
    let actual_existing = bound().unwrap().0;
    assert_eq!(
        existing
            .root
            .domain_usage()
            .unwrap()
            .retained_data_reservation,
        actual_existing + measured_new.bytes_total as usize,
        "actual Root+9queues+8signal anchors+Status DATA/routes/role metadata must all survive old/new cell overlap, once each"
    );
}

#[test]
fn actual_factory_status_roles_follow_worker_native_carriers_through_real_retirement() {
    let (control, callback) =
        channel_with_factory_reservation(48_000, None, Some(reservation(1 << 20, 2 << 20)))
            .unwrap();
    let root = control.native_candidate_root();
    let expected = status_owner::StatusArchive::requested_bytes().unwrap();
    assert_eq!(root.test_status_data().unwrap(), expected);
    assert_eq!(
        root.test_resident_facts().unwrap(),
        (CandidateRoot::resident_requested_bytes().unwrap(), 2)
    );
    let (mut data, errors, terminals, client, worker) =
        crate::native_callback_owner::reserve_native_worker(control, callback, None).unwrap();
    assert_eq!(root.test_status_data().unwrap(), expected);
    assert_eq!(
        root.test_resident_facts().unwrap(),
        (CandidateRoot::resident_requested_bytes().unwrap(), 6)
    );
    let mut output = [[1.0; 2]; 128];
    let measured = allocation_counter::measure(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.render(&mut output).unwrap());
    });
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
    let mut retirement =
        crate::native_callback_owner::NativeRetirement::new(terminals, root.clone());
    drop((client, worker, data, errors));
    assert!(retirement.advance().unwrap());
    assert_eq!(
        root.test_resident_facts().unwrap(),
        (CandidateRoot::resident_requested_bytes().unwrap(), 2)
    );
    assert_eq!(root.test_status_data().unwrap(), expected);
    drop(retirement);
    assert_eq!(root.test_status_data().unwrap(), expected);
    eprintln!(
        "SUPPORTED actual factory Status roles across real Client/Worker/Data/Error carriers + callback0alloc/free + actual NativeRetirement; Root anchor remains. This silence fixture is not active Sampler/native publication admission"
    );
}

#[test]
fn actual_foreign_prepared_queue_refuses_before_native_carrier_birth() {
    let mut completed = 0;
    for field in 0..6 {
        let (retained, peak) = native_birth_bound().unwrap();
        let (mut control, mut callback) =
            channel_with_factory_reservation(48000, None, Some(reservation(retained, peak)))
                .unwrap();
        let root = control.native_candidate_root();
        let (foreign_publications, foreign_incoming) = native_prepared_ring::<QueuedPublication>(1);
        let (foreign_returning, foreign_retired) =
            native_prepared_ring::<ReturnedPublication>(RETIRED);
        let (foreign_commands, foreign_events) = native_prepared_ring::<QueuedCommand>(COMMANDS);
        // Replace each REAL owning endpoint in turn, not its scalar slot index.
        // Same element type/capacity never supplies actual constructor lineage.
        match field {
            0 => callback.publications = foreign_incoming,
            1 => control.publication.publications = foreign_publications,
            2 => callback.retired = foreign_returning,
            3 => control.publication.retired = Some(foreign_retired),
            4 => callback.commands = foreign_events,
            5 => control.commands = foreign_commands,
            _ => unreachable!(),
        }
        let mut refused = false;
        let birth = allocation_counter::measure(|| {
            refused = crate::native_callback_owner::reserve_native_worker(control, callback, None)
                .is_err();
        });
        assert!(
            refused,
            "foreign prepared endpoint {field} cannot inherit Root origin"
        );
        assert_eq!(
            birth.count_total, 0,
            "field {field}: refuse before carrier Box/cell/record births"
        );
        assert_eq!(root.test_native_carrier_facts().unwrap().0, 0);
        completed += 1;
        eprintln!(
            "SUPPORTED foreign prepared queue actualfield{field} refused before any carrier allocation"
        );
    }
    assert_eq!(completed, 6);
    eprintln!(
        "SUPPORTED all6 actual prepared endpoint substitutions completed; no owning element descendants/admission inferred"
    );
}

#[test]
fn actual_foreign_nested_queue_fields_refuse_before_all_native_carrier_births() {
    let mut completed = 0;
    for field in 0..8 {
        let (retained, peak) = native_birth_bound().unwrap();
        let (mut control, mut callback) =
            channel_with_factory_reservation(48000, None, Some(reservation(retained, peak)))
                .unwrap();
        let root = control.native_candidate_root();
        // Real owning sub-endpoints with identical public behavior/layout.
        // Their actual queue fields belong to a different fresh constructor.
        match field {
            0 => control.spectrum_target = spectrum_target::channel().0,
            1 => callback.spectrum_target = spectrum_target::channel().1,
            2 => control.processor_audition = processor_audition::channel().0,
            3 => callback.processor_audition = processor_audition::channel().1,
            4 => control.sample_preview = crate::sample_preview::channel(48000).0,
            5 => callback.sample_preview = crate::sample_preview::channel(48000).1,
            6 => control.gain_recording = crate::gain_recording::channel().0,
            7 => callback.gain_recording = crate::gain_recording::channel().1,
            _ => unreachable!(),
        }
        let mut refused = false;
        let birth = allocation_counter::measure(|| {
            refused = crate::native_callback_owner::reserve_native_worker(control, callback, None)
                .is_err();
        });
        assert!(
            refused,
            "foreign nested owning endpoint {field} cannot inherit Root queue origin"
        );
        assert_eq!(
            birth.count_total, 0,
            "nested field {field}: refuse before all native carrier births"
        );
        assert_eq!(root.test_native_carrier_facts().unwrap().0, 0);
        completed += 1;
        eprintln!("SUPPORTED actualnestedfield{field} foreign queue refused before carrier birth");
    }
    assert_eq!(completed, 8);
}

#[test]
fn actual_foreign_callback_status_refuses_before_any_native_carrier_birth() {
    let mut completed = 0;
    for field in 0..2 {
        let (retained, peak) = native_birth_bound().unwrap();
        let (mut control, mut callback) =
            channel_with_factory_reservation(48000, None, Some(reservation(retained, peak)))
                .unwrap();
        let root = control.native_candidate_root();
        // All actual queue/atomic fields stay intact in BOTH variants.
        if field == 0 {
            callback.status =
                status_owner::StatusHandle::legacy_fixture(Status::try_new().unwrap());
        } else {
            // Same physical Status, actual paired leases, wrong owning fields.
            std::mem::swap(&mut control.publication.status, &mut callback.status);
        }
        let mut refused = false;
        let birth = allocation_counter::measure(|| {
            refused = crate::native_callback_owner::reserve_native_worker(control, callback, None)
                .is_err();
        });
        assert!(
            refused,
            "Status field {field} must refuse before creating an unbound native bundle"
        );
        assert_eq!(birth.count_total, 0);
        assert_eq!(root.test_native_carrier_facts().unwrap().0, 0);
        completed += 1;
        eprintln!("SUPPORTED actual Status fieldvariant{field} refused before all carrier births");
    }
    assert_eq!(completed, 2);
}

#[test]
fn actual_foreign_resident_field_refuses_without_overwrite_or_carrier_birth() {
    let (retained, peak) = native_birth_bound().unwrap();
    let (mut control, callback) =
        channel_with_factory_reservation(48000, None, Some(reservation(retained, peak))).unwrap();
    let root = control.native_candidate_root();
    // Actual present owner, not a forged None/empty-body certificate. The
    // foreign slot is empty here; requests/runtime custody remains independent.
    control.publication.resident_forwarding = Some(resident_forwarding::ResidentHandle::Legacy(
        resident_forwarding::ResidentForwarding::new(),
    ));
    let mut refused = false;
    let birth = allocation_counter::measure(|| {
        refused =
            crate::native_callback_owner::reserve_native_worker(control, callback, None).is_err();
    });
    assert!(
        refused,
        "a present foreign forwarding owner must not be overwritten and silently accepted"
    );
    assert_eq!(birth.count_total, 0);
    assert_eq!(root.test_native_carrier_facts().unwrap().0, 0);
}

#[test]
fn actual_root_actor_role_swap_refuses_before_any_native_carrier_birth() {
    let (retained, peak) = native_birth_bound().unwrap();
    let (mut control, mut callback) =
        channel_with_factory_reservation(48000, None, Some(reservation(retained, peak))).unwrap();
    let root = control.native_candidate_root();
    // Same physical Root and both genuine LIVE leases: only the actual owning
    // fields are exchanged. Neither root ID nor any-role membership detects it.
    std::mem::swap(
        &mut control.publication.candidate_root,
        &mut callback.candidate_root,
    );
    let mut refused = false;
    let birth = allocation_counter::measure(|| {
        refused =
            crate::native_callback_owner::reserve_native_worker(control, callback, None).is_err();
    });
    assert!(
        refused,
        "same Root actor leases in wrong fields must refuse before native escape"
    );
    assert_eq!(birth.count_total, 0);
    assert_eq!(root.test_native_carrier_facts().unwrap().0, 0);
    eprintln!(
        "SUPPORTED actual sameRoot publication/callback actor role swap refused before all carrier births"
    );
}
