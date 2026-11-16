//! Real closed-native construction/publication; generic Playback alone is not evidence.
use super::*;
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{Command, Created, Editor};
fn project(grouped: bool) -> (hex_project::Project, hex_project::TrackId) {
    let (base, track) = utility_fixture::project();
    let mut snapshot = base.snapshot();
    snapshot.tracks[0].devices[0].state = hex_devices::reverb::REVERB_DEFINITION
        .default_state()
        .unwrap();
    let mut edit = Editor::new(hex_project::Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    let Created::ReturnTrack(bus) = edit
        .execute(Command::AddReturnTrack {
            name: "room return".into(),
        })
        .unwrap()
    else {
        panic!("return");
    };
    edit.execute(Command::InsertReturnDevice {
        return_track: bus,
        before: None,
        state: hex_devices::reverb::REVERB_DEFINITION
            .default_state()
            .unwrap(),
    })
    .unwrap();
    edit.execute(Command::SetReturnSend {
        track,
        return_track: bus,
        gain: 0.4,
    })
    .unwrap();
    if grouped {
        let Created::Track(group) = edit
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        edit.execute(Command::Gain {
            track: group,
            gain: 0.5,
        })
        .unwrap();
    }
    (edit.project().clone(), track)
}
fn owner(
    project: &hex_project::Project,
) -> (hex_engine::AudioCandidateOwner, (usize, usize, u64, u64)) {
    let asset = project.assets().next().unwrap().id();
    let bank = hex_engine::OwnedAudioInputs::try_new(
        vec![(
            asset,
            hex_engine::OwnedAudioSource::try_resident(48000, vec![[0.25, -0.1]; 8])
                .ok()
                .unwrap(),
        )],
        hex_engine::AudioStorageReservation {
            allocation_items: 256,
            handle_obligations: 1024,
            max_prepared_data_bytes: 96 * 1024 * 1024,
            max_construction_data_bytes: 96 * 1024 * 1024,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap();
    let bank_id = bank.test_bank_identity();
    let registry = hex_devices::native_registry().unwrap();
    let mut custody = hex_engine::AudioPreparationCustody::try_new_on_control().unwrap();
    let token = custody
        .begin_input_on_control(
            hex_engine::AudioArrangementInput::new(project, 11, bank)
                .with_effect_registry(&registry)
                .with_audio_prepared_revision(),
        )
        .ok()
        .unwrap();
    (
        hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap(),
        bank_id,
    )
}
fn zero_counts(counts: allocation_counter::AllocationInfo) {
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
#[test]
fn actual_native_reverb_track_return_pcm_freeze_tail_moves_and_control_reclamation() {
    native_reverb_case(false);
}
#[test]
fn grouped_native_reverb_tail_moves_through_group_without_callback_allocation() {
    native_reverb_case(true);
}

#[test]
fn disconnect_return_send_keeps_real_tail_in_callback_without_allocation() {
    let _serial = serial();
    let (base, track) = project(false);
    let mut edit = Editor::new(base, 16).unwrap();
    let track_device = edit.project().track(track).unwrap().devices()[0].id();
    edit.execute(Command::RemoveDevice(track_device)).unwrap();
    edit.execute(Command::InsertMainDevice {
        before: None,
        state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
            "hex.limiter",
        ))
        .unwrap(),
    })
    .unwrap();
    let a_project = edit.project().clone();
    let bus = a_project.return_tracks()[0].id();
    let device = a_project.return_tracks()[0].devices()[0].id();
    edit.execute(Command::RemoveDevice(device)).unwrap();
    let b_project = edit.project().clone();
    edit.execute(Command::SetReturnSend {
        track,
        return_track: bus,
        gain: 0.0,
    })
    .unwrap();
    let c_project = edit.project().clone();
    let asset = a_project.assets().next().unwrap().id();
    let media = std::collections::BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48000, vec![[0.25, -0.1]; 8]).unwrap(),
    )]);
    let prepare = |project: &hex_project::Project| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(
                project,
                &media,
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    };
    let playback = |project: &hex_project::Project| {
        hex_engine::playback::Playback::try_from_prepared_with_session(prepare(project)).unwrap()
    };
    let mut reference = playback(&a_project);
    reference.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(playback(&a_project)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut actual = vec![[0.0; 2]; 4096];
    let mut expected = actual.clone();
    reference.render(&mut expected).unwrap();
    zero_counts(allocation_counter::measure(|| {
        callback.begin_block();
        for block in actual.chunks_mut(127) {
            callback.render(block).unwrap();
        }
    }));
    assert_eq!(actual, expected);
    control.reclaim();

    let b_revision = control.publish(playback(&b_project)).unwrap();
    let mut actual = [[0.0; 2]; 127];
    let mut expected = actual;
    reference.render(&mut expected).unwrap();
    zero_counts(allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    assert_eq!(control.status.applied_revision.load(Acquire), b_revision);
    assert_eq!(actual, expected);
    assert!(control.reclaim() > 0);

    let c_revision = control.publish(playback(&c_project)).unwrap();
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    reference.render(&mut expected).unwrap();
    zero_counts(allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    assert_eq!(control.status.applied_revision.load(Acquire), c_revision);
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    assert!(control.reclaim() > 0);
    control.request_shutdown();
    zero_counts(allocation_counter::measure(|| callback.begin_block()));
    assert!(control.reclaim() > 0);
}

fn native_reverb_case(grouped: bool) {
    use hex_lattice_contract::ParameterValue;
    let _serial = serial();
    assert!(hex_engine::audio_storage_constructor_layout_supported());
    let (a_project, track) = project(grouped);
    let mut edit = Editor::new(a_project.clone(), 16).unwrap();
    let devices: Vec<_> = edit
        .project()
        .track(track)
        .unwrap()
        .devices()
        .iter()
        .chain(edit.project().return_tracks()[0].devices())
        .map(|d| d.id())
        .collect();
    for device in &devices {
        let changed = hex_engine::patch_native_device_parameter(
            edit.project().device(*device).unwrap().state(),
            hex_devices::reverb::parameter::FREEZE,
            ParameterValue::Bool(true),
        )
        .unwrap();
        edit.execute(Command::ReplaceDeviceState {
            device: *device,
            state: changed,
        })
        .unwrap();
    }
    let b_project = edit.project().clone();
    for device in devices {
        edit.execute(Command::RemoveDevice(device)).unwrap();
    }
    let c_project = edit.project();
    let (mut a, a_bank) = owner(&a_project);
    let (b, b_bank) = owner(&b_project);
    let (c, c_bank) = owner(c_project);
    let limits = reservation(&mut a);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let a_ticket = control
        .stage_eligible_audio_candidate(a, vec![], PublicationPolicy::InheritRuntime, 15)
        .unwrap_or_else(|refusal| panic!("A native stage: {}", refusal.reason));
    control.command(PlaybackCommand::Play).unwrap();
    let asset = a_project.assets().next().unwrap().id();
    let media = std::collections::BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48000, vec![[0.25, -0.1]; 8]).unwrap(),
    )]);
    let revision = hex_engine::PreparedRevision::prepare(
        &a_project,
        &media,
        Arc::new(hex_engine::SineTable::new()),
        None,
    )
    .unwrap();
    let mut reference =
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap();
    reference.apply(PlaybackCommand::Play);
    let mut expected = vec![[0.0; 2]; 4096];
    reference.render(&mut expected).unwrap();
    let mut actual = expected.clone();
    zero_counts(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            for block in actual.chunks_mut(127) {
                callback.render(block).unwrap();
            }
        })
    }));
    assert!(
        actual == expected,
        "closed native output equals the registered processor path"
    );
    control.reclaim();
    let b_ticket = control
        .stage_eligible_audio_candidate(b, vec![], PublicationPolicy::InheritRuntime, 15)
        .unwrap_or_else(|refusal| panic!("B native stage: {}", refusal.reason));
    let mut block = [[0.0; 2]; 127];
    let mut b_boxes = (None, None);
    zero_counts(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut block).unwrap();
            let Some(ActivePublication::Eligible(active)) = callback.active.as_ref() else {
                panic!("B");
            };
            b_boxes = active.runtime().unwrap().test_track_boxes(track).unwrap();
        })
    }));
    assert!(b_boxes.0.is_some() && b_boxes.1.is_some());
    control.reclaim();
    // B's parameter replacement copies A's histories rather than moving its
    // origins. Reclaim schedules asynchronous cleanup; recover A on control
    // before staging C, then wait for the executor to release its work lock.
    // B retains the frozen histories whose actual Box moves are checked below.
    let a_recovery = recover(&mut control, &a_ticket);
    let root = control.native_candidate_root();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while root.cleanup_scheduled() {
        assert!(
            std::time::Instant::now() < deadline,
            "executor retained A's completed cleanup job"
        );
        std::thread::yield_now();
    }
    let c_ticket = control
        .stage_eligible_audio_candidate(c, vec![], PublicationPolicy::InheritRuntime, 15)
        .unwrap_or_else(|refusal| panic!("C native stage: {}", refusal.reason));
    zero_counts(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            let Some(ActivePublication::Eligible(active)) = callback.active.as_ref() else {
                panic!("C");
            };
            assert_eq!(
                active.runtime().unwrap().test_tail_boxes(track),
                Some(b_boxes)
            );
            for _ in 0..40 {
                callback.render(&mut block).unwrap();
            }
        })
    }));
    assert!(
        block.iter().flatten().any(|v| v.abs() > 1e-8),
        "removed frozen histories continue through original native tail owners"
    );
    control.command(PlaybackCommand::Stop).unwrap();
    zero_counts(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            // The queued Stop fades over 128 frames before it resets (#4151).
            for _ in 0..2 {
                callback.begin_block();
                callback.render(&mut block).unwrap();
            }
        })
    }));
    assert_eq!(block, [[0.0; 2]; 127]);
    control.reclaim();
    drop(data);
    drop(errors);
    retirement = std::thread::spawn(move || {
        let mut retirement = retirement;
        let mut stream = Some(());
        let mut pending = false;
        let mut disposed = false;
        crate::lifecycle::test_dispose_backend(
            &mut stream,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
        .unwrap();
        assert!(stream.is_none() && disposed);
        retirement
    })
    .join()
    .unwrap();
    assert_eq!(a_recovery.inputs.test_bank_identity(), a_bank);
    assert_eq!(
        recover(&mut control, &b_ticket).inputs.test_bank_identity(),
        b_bank
    );
    assert_eq!(
        recover(&mut control, &c_ticket).inputs.test_bank_identity(),
        c_bank
    );
    drop(control);
    finish(&mut retirement);
    eprintln!(
        "original native reverb track/return: PCM, zero total/net callback allocations, real tail Box moves, final bank recovery PASS"
    );
}
