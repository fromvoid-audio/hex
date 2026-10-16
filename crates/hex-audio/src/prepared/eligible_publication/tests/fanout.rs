use super::*;
#[test]
fn actual_native_abc_moves_independent_utility_subsets_and_reclaims_each_original_bank() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (a_project, [x, y, z]) = utility_fixture::abc_project();
    let mut edit = hex_project::Editor::new(a_project.clone(), 8).unwrap();
    edit.execute(hex_project::Command::RemoveTrack(x)).unwrap();
    edit.execute(hex_project::Command::RemoveTrack(y)).unwrap();
    let b_project = edit.project();
    let (mut a, a_bank) = utility_fixture::owner_with_bank(&a_project);
    let a_id = a.identity();
    let (b, b_bank) = utility_fixture::owner_with_bank(b_project);
    let b_id = b.identity();
    let (c, c_bank) = utility_fixture::owner_with_bank(b_project);
    let c_id = c.identity();
    let reservation = reservation(&mut a);
    let (mut control, mut data, errors, retirement) = anchored();
    control
        .configure_audio_candidate_publication(reservation)
        .unwrap();
    let a_ticket = control
        .stage_eligible_audio_candidate(a, vec![], PublicationPolicy::InheritRuntime, 15)
        .ok()
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut original_boxes = [(None, None); 3];
    let mut runtime_a = 0;
    let mut prepared_a = 0;
    data.owner_mut().unwrap().run(|callback, _| {
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 8]).unwrap();
        let Some(ActivePublication::Eligible(active)) = callback.active.as_ref() else {
            panic!("active A")
        };
        let runtime = active.runtime().unwrap();
        runtime_a = runtime.test_runtime_address().unwrap();
        prepared_a = runtime.test_prepared_strong_count().unwrap();
        original_boxes = [
            runtime.test_track_boxes(x).unwrap(),
            runtime.test_track_boxes(y).unwrap(),
            runtime.test_track_boxes(z).unwrap(),
        ];
    });
    assert!(
        original_boxes
            .iter()
            .all(|(a, b)| a.is_some() && b.is_some() && a != b)
    );
    let b_ticket = control
        .stage_eligible_audio_candidate(b, vec![], PublicationPolicy::InheritRuntime, 15)
        .ok()
        .unwrap();
    let root = control.native_candidate_root();
    let handles = root.test_strong_count();
    let mut prepared_b = 0;
    let mut runtime_b = 0;
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            let Some(ActivePublication::Eligible(active)) = callback.active.as_ref() else {
                panic!("active B")
            };
            let runtime = active.runtime().unwrap();
            runtime_b = runtime.test_runtime_address().unwrap();
            prepared_b = runtime.test_prepared_strong_count().unwrap();
            assert_eq!(runtime.test_tail_boxes(x), Some(original_boxes[0]));
            assert_eq!(runtime.test_tail_boxes(y), Some(original_boxes[1]));
            assert_ne!(runtime.test_track_boxes(z), Some(original_boxes[2])); // B history copy is not A origin
        });
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
    assert_eq!(root.test_strong_count(), handles); // callback did not clone Root handles
    let ReturnedPublication::Eligible(returned) = control.retired.as_ref().unwrap().peek().unwrap()
    else {
        panic!("return A")
    };
    assert_eq!(returned.test_manifest_counts(a_id), Some((2, 2)));
    assert_eq!(returned.test_runtime_address(), Some(runtime_a));
    assert_eq!(returned.test_track_boxes(z), Some(original_boxes[2]));
    // Queued acceptance/ACK did not dispose A or authorize original-bank reuse.
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    assert!(control.recover_audio_candidate(&a_ticket).is_none());
    control.reclaim();
    data.owner_mut().unwrap().run(|callback, _| {
        callback.render(&mut [[0.0; 2]; 4096]).unwrap();
        let Some(ActivePublication::Eligible(active)) = callback.active.as_ref() else {
            panic!("active B")
        };
        let runtime = active.runtime().unwrap();
        assert_eq!(runtime.test_runtime_address(), Some(runtime_b));
        assert_eq!(runtime.test_prepared_strong_count(), Some(prepared_b));
        assert_eq!(runtime.test_tail_activity(x), Some((true, true)));
        assert_eq!(runtime.test_tail_activity(y), Some((false, false)));
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !root.test_return_metadata_dropped(&a_ticket) {
        assert!(
            std::time::Instant::now() < deadline,
            "A runtime/metadata cleanup did not progress autonomously"
        );
        std::thread::yield_now();
    }
    let c_ticket = control
        .stage_eligible_audio_candidate(c, vec![], PublicationPolicy::InheritRuntime, 15)
        .ok()
        .unwrap();
    let handles = root.test_strong_count();
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            let Some(ActivePublication::Eligible(active)) = callback.active.as_ref() else {
                panic!("active C")
            };
            let runtime = active.runtime().unwrap();
            assert_eq!(runtime.test_tail_boxes(x), Some(original_boxes[0]));
            assert_eq!(runtime.test_tail_boxes(y), None);
        });
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
    assert_eq!(root.test_strong_count(), handles);
    let ReturnedPublication::Eligible(returned) = control.retired.as_ref().unwrap().peek().unwrap()
    else {
        panic!("return B")
    };
    assert_eq!(returned.test_manifest_counts(a_id), Some((1, 1)));
    assert_eq!(returned.test_runtime_address(), Some(runtime_b));
    assert!(returned.test_track_boxes(z).is_some());
    let b_ready = recover(&mut control, &b_ticket);
    assert_eq!(b_ready.disposed.identity(), b_id);
    assert_eq!(b_ready.inputs.test_bank_identity(), b_bank);
    assert_eq!(
        b_ready.return_kind,
        crate::AudioCandidateReturnKind::Replaced
    );
    assert!(control.recover_audio_candidate(&a_ticket).is_none()); // x is still a real C owner
    assert!(control.recover_audio_candidate(&c_ticket).is_none());
    assert!(prepared_a > 0 && prepared_b > 0);
    // Normal completed backend handle plus both final guard releases closes C;
    // only then can the original A x origin and A bank complete independently.
    drop(data);
    drop(errors);
    let mut retirement = std::thread::spawn(move || {
        let mut retirement = retirement;
        let mut stream = Some(());
        let mut pending = false;
        let mut disposed = false;
        let _ = crate::lifecycle::test_dispose_backend(
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
    let a_ready = recover(&mut control, &a_ticket);
    let c_ready = recover(&mut control, &c_ticket);
    assert_eq!(a_ready.disposed.identity(), a_id);
    assert_eq!(c_ready.disposed.identity(), c_id);
    assert_eq!(a_ready.inputs.test_bank_identity(), a_bank);
    assert_eq!(c_ready.inputs.test_bank_identity(), c_bank);
    assert_eq!(
        c_ready.return_kind,
        crate::AudioCandidateReturnKind::FinalActive
    );
    assert!(control.recover_audio_candidate(&a_ticket).is_none());
    assert!(control.recover_audio_candidate(&b_ticket).is_none());
    assert!(control.recover_audio_candidate(&c_ticket).is_none());
    drop((a_ready, b_ready, c_ready));
    drop(control);
    finish(&mut retirement);
}
