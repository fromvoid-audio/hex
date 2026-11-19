//! A retained tail cannot borrow the same physical native endpoint as the
//! current chain. Refusal must precede graph acceptance and preserve old PCM.
use super::*;
use hex_lattice_contract::NativeDeviceMetadata;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_main_reorder_cannot_process_one_endpoint_as_current_and_tail() {
    let _serial = serial();
    let PreparedFixture {
        mut control,
        mut data,
        errors,
        retirement,
        binding,
        owner,
        project,
        samples,
        ..
    } = prepared_fixture_with_profile(2, false, "processing", 8, false);
    let root = control.native_candidate_root();
    let first = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 41)
        .unwrap_or_else(|r| panic!("initial native graph: {}", r.reason));
    control.command(PlaybackCommand::Play).unwrap();
    let native = project.main_devices()[0].id();
    let mut editor = Editor::new(project, 8).unwrap();
    let mut utility = None;
    let mut second = None;
    let mut third = None;
    let mut fourth = None;
    let mut output = [[0.0; 2]; 96];
    for (block, chunk) in output.chunks_mut(8).enumerate() {
        if block == 1 {
            let Created::Device(device) = editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
                        .unwrap(),
                })
                .unwrap()
            else {
                panic!("utility")
            };
            utility = Some(device);
            second = Some(replacement_preparation(
                &mut control,
                &[binding],
                editor.project(),
                &samples,
            ));
        } else if block == 2 {
            editor
                .execute(Command::MoveMainDevice {
                    device: utility.unwrap(),
                    before: Some(native),
                })
                .unwrap();
            assert_eq!(editor.project().main_devices()[0].id(), utility.unwrap());
            assert_eq!(editor.project().main_devices()[1].id(), native);
            third = Some(replacement_preparation(
                &mut control,
                &[binding],
                editor.project(),
                &samples,
            ));
        } else if block == 8 {
            // The real accepted zero-input prefix has now drained the native
            // bridge. Reordering no longer needs a second tail consumer and
            // must remain legal with this same original endpoint.
            fourth = Some(replacement_preparation(
                &mut control,
                &[binding],
                editor.project(),
                &samples,
            ));
        }
        let mut host = [0.0_f32; 16];
        allocation_free(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.begin_native_effect_callback(8).unwrap();
                let rendered = callback.render(chunk);
                assert!(
                    rendered.is_ok(),
                    "reordered original source at block {block}: {rendered:?}"
                );
                assert!(crate::device::finish_host_chunk(
                    callback,
                    &mut host,
                    chunk,
                    2,
                    &rendered,
                    block as u64 * 8,
                    Instant::now(),
                ));
            })
        });
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            if block == 0 {
                1
            } else if block < 8 {
                2
            } else {
                4
            }
        );
        replacement::wait_for_blocks(&root, binding, block as u64 + 1);
        if block == 1 {
            let returned = crate::prepared::candidate_tests::recover(&mut control, &first);
            assert_eq!(
                returned.return_kind,
                crate::AudioCandidateReturnKind::Replaced
            );
            drop(returned);
        } else if block == 2 {
            let returned =
                crate::prepared::candidate_tests::recover(&mut control, third.as_ref().unwrap());
            assert_eq!(
                returned.return_kind,
                crate::AudioCandidateReturnKind::NeverInstalled
            );
            drop(returned);
        } else if block == 8 {
            let returned =
                crate::prepared::candidate_tests::recover(&mut control, second.as_ref().unwrap());
            assert_eq!(
                returned.return_kind,
                crate::AudioCandidateReturnKind::Replaced
            );
            drop(returned);
        }
    }
    let mut expected = [[0.0; 2]; 96];
    for (index, sample) in samples.iter().enumerate() {
        let gain = 0.5 + (index / 8) as f32 * 0.125;
        expected[index + 16] = [sample[0] * gain, sample[1] * gain];
    }
    assert_eq!(output, expected);
    let mut retirement = close_backend(data, errors, retirement);
    let returned =
        crate::prepared::candidate_tests::recover(&mut control, fourth.as_ref().unwrap());
    assert_eq!(
        returned.return_kind,
        crate::AudioCandidateReturnKind::FinalActive
    );
    drop(returned);
    let returned = recover(&mut control, binding);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
        panic!("original reorder source disposal")
    };
    assert!(disposal.status.success());
    let terminal = disposal.terminal.unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (12, 96));
    drop(control);
    finish(&mut retirement);
}
