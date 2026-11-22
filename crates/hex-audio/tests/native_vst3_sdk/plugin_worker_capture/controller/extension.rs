//! Actual original worker completion, distinct from Root capability publication.
use super::*;
use crate::plugin_worker::NativeWorkerAuthoredParameterProgress as Progress;

fn begin_extension(
    owner: &mut NativeWorker,
    class: hex_plugin_state::ClassId,
    steps: u32,
    q: ParameterPageQuery,
) -> NativeWorkerControllerQuery {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.begin_authored_parameter_extension_on_control(class, 7, steps, q) {
            Ok(token) => return token,
            Err(
                "native controller mailbox is busy"
                | "native preparation is busy"
                | "native controller query is already pending",
            ) => {}
            Err(reason) => panic!("extension admission: {reason}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}

fn completed(owner: &mut NativeWorker, token: &NativeWorkerControllerQuery) -> Progress {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let progress = owner
            .poll_authored_parameter_extension_on_control(token)
            .unwrap();
        if progress != Progress::Pending {
            return progress;
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn original_worker_extensions_preserve_pcm_storage_and_exact_terminal_custody() {
    const NAME: &str = "plugin_worker::capture::tests::controller::extension::original_worker_extensions_preserve_pcm_storage_and_exact_terminal_custody";
    if fixture_child(91, NAME) {
        return;
    }
    for reserved in [false, true] {
        let mut instance = original_instance();
        let child = instance.child_id_on_control();
        let class = instance.configuration().identity.class;
        if reserved {
            instance
                .reserve_authored_parameter_capacity_on_control(1)
                .unwrap();
        }
        let (mut owner, mut callback) = NativeWorker::start_on_control(instance).unwrap();
        let storage = owner.constructor_storage_on_control(&callback).unwrap();
        let ordinary = query(&mut owner);
        assert!(
            owner
                .poll_authored_parameter_extension_on_control(&ordinary)
                .is_err()
        );
        let observed = page(&mut owner, &ordinary);
        let q = ParameterPageQuery {
            first: observed.first,
            revision: observed.revision,
            conversion: None,
        };
        let wrong = begin_extension(
            &mut owner,
            hex_plugin_state::ClassId::new([9; 16]).unwrap(),
            0,
            q,
        );
        assert!(matches!(
            completed(&mut owner, &wrong),
            Progress::Refused(_)
        ));
        let wrong_steps = begin_extension(&mut owner, class, 1, q);
        assert!(matches!(
            completed(&mut owner, &wrong_steps),
            Progress::Refused(_)
        ));

        let token = begin_extension(&mut owner, class, 0, q);
        assert!(owner.begin_controller_query_on_control(q).is_err());
        assert!(
            owner.poll_controller_query_on_control(&token).is_err(),
            "wrong-kind polls cannot consume the terminal"
        );
        let (mut other, other_callback) =
            NativeWorker::start_on_control(original_instance()).unwrap();
        assert!(
            other
                .poll_authored_parameter_extension_on_control(&token)
                .is_err(),
            "equal bindings cannot substitute another completion owner"
        );
        other.revoke_on_control();
        assert!(other.retire_on_control().unwrap().failure.is_none());
        assert!(other.owns_callback(&other_callback));
        let capture = begin(&mut owner);
        for sequence in 0..3 {
            callback_block(&owner, &mut callback, sequence);
        }
        let progress = completed(&mut owner, &token);
        assert_eq!(
            matches!(
                progress,
                Progress::Armed {
                    id: 7,
                    steps: 0,
                    ..
                }
            ),
            reserved
        );
        if !reserved {
            assert!(matches!(progress, Progress::Refused(_)));
        }
        assert!(
            owner
                .poll_authored_parameter_extension_on_control(&token)
                .is_err()
        );
        complete(&mut owner, &capture);
        assert_eq!(
            owner.constructor_storage_on_control(&callback),
            Some(storage)
        );
        let late = begin_extension(&mut owner, class, 0, q);
        owner.revoke_on_control();
        assert_eq!(completed(&mut owner, &late), Progress::Failed);
        let stopped = begin_extension(&mut owner, class, 0, q);
        let progress = completed(&mut owner, &stopped);
        assert_eq!(matches!(progress, Progress::Armed { .. }), reserved);
        assert!(!owner.transport_is_live_on_control());
        assert!(owner.owns_callback(&callback));
        assert_eq!(
            owner.constructor_storage_on_control(&callback),
            Some(storage)
        );
        let quiescent = owner.quiesce_on_control().unwrap();
        assert_eq!(quiescent.child_id_on_control(), child);
        assert_eq!(quiescent.completed_blocks(), 3);
        assert!(quiescent.retire_on_control().unwrap().status.success());
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn retiring_a_held_extension_cancels_the_same_original_reader() {
    const NAME: &str = "plugin_worker::capture::tests::controller::extension::retiring_a_held_extension_cancels_the_same_original_reader";
    if fixture_child(93, NAME) {
        return;
    }
    let mut instance = original_instance();
    let class = instance.configuration().identity.class;
    instance
        .reserve_authored_parameter_capacity_on_control(1)
        .unwrap();
    let (mut owner, mut callback) = NativeWorker::start_on_control(instance).unwrap();
    let token = begin_extension(
        &mut owner,
        class,
        0,
        ParameterPageQuery {
            revision: 1,
            ..Default::default()
        },
    );
    let until = Instant::now() + Duration::from_secs(3);
    while fs::read(trace_path()).ok().is_none_or(|bytes| {
        bytes.len() != 56 || i32::from_le_bytes(bytes[..4].try_into().unwrap()) != 26
    }) {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        owner
            .poll_authored_parameter_extension_on_control(&token)
            .unwrap(),
        Progress::Pending
    );
    for sequence in 0..3 {
        callback_block(&owner, &mut callback, sequence);
    }
    owner.revoke_on_control();
    assert_eq!(completed(&mut owner, &token), Progress::Failed);
    assert!(matches!(
        owner.retire_on_control().unwrap().failure,
        Some(hex_plugin_host::Error::Cancelled)
    ));
    drop(owner);
    assert!(token.original_worker_released_on_control());
}
