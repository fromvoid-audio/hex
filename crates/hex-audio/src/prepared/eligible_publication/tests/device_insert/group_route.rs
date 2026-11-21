//! A verified Group route cut keeps both source windows live through the leaf fade.
use super::*;
use hex_devices::utility::{UTILITY_DEFINITION, parameter::INVERT_LEFT};
use hex_engine::MediaPages as _;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Created, TrackDestination, TrackSelectionIds};

const FADE: usize = 240;

enum RouteDirection {
    Enter,
    Exit,
    EnterEmpty,
    EnterEmptyWithDelay,
    ExitLastChild,
    ExitLastChildWithDelay,
}

#[derive(Clone, Copy)]
enum RouteEnd {
    Install,
    Cancel,
}

fn sample(frame: u32) -> i16 {
    i16::try_from(1024 + (frame * 13) % 4096).unwrap()
}

fn assert_frame(frame: u64, actual: Frame, leaf: f32, leaf_grouped: bool, sibling_grouped: bool) {
    // Both leaves use the same verified asset. The unaffected quarter-gain
    // sibling keeps its own route for the full edit.
    let source = f32::from(sample(u32::try_from(frame).unwrap())) / 32768.0;
    let sibling = source * 0.25;
    let sibling_left = if sibling_grouped { -sibling } else { sibling };
    let leaf_left = if leaf_grouped { -leaf } else { leaf };
    let expected = [sibling_left + leaf_left, sibling + leaf];
    assert!(
        actual
            .into_iter()
            .zip(expected)
            .all(|(actual, expected)| (actual - expected).abs() < 2e-6),
        "frame {frame}: {actual:?} != {expected:?}"
    );
}

#[test]
fn verified_group_entry_fades_only_the_leaf_across_warm_file_pages() {
    let _serial = serial();
    for partition in [1, 257] {
        check_group_route(partition, RouteDirection::Enter, RouteEnd::Install);
    }
}

#[test]
fn cancelling_a_verified_group_entry_restores_its_original_route() {
    let _serial = serial();
    check_group_route(257, RouteDirection::Enter, RouteEnd::Cancel);
}

#[test]
fn verified_group_exit_keeps_the_original_until_the_exact_receipt() {
    let _serial = serial();
    for partition in [1, 257] {
        check_group_route(partition, RouteDirection::Exit, RouteEnd::Install);
    }
}

#[test]
fn cancelling_a_verified_group_exit_restores_its_original_route() {
    let _serial = serial();
    check_group_route(257, RouteDirection::Exit, RouteEnd::Cancel);
}

#[test]
fn verified_first_child_entry_fades_across_warm_file_pages() {
    let _serial = serial();
    for partition in [1, 257] {
        check_group_route(partition, RouteDirection::EnterEmpty, RouteEnd::Install);
    }
}

#[test]
fn cancelling_a_verified_first_child_entry_preserves_the_flat_source() {
    let _serial = serial();
    check_group_route(257, RouteDirection::EnterEmpty, RouteEnd::Cancel);
}

#[test]
fn verified_first_child_entry_starts_the_leaf_with_fresh_delay_history() {
    let _serial = serial();
    for end in [RouteEnd::Install, RouteEnd::Cancel] {
        check_group_route(257, RouteDirection::EnterEmptyWithDelay, end);
    }
}

#[test]
fn verified_last_child_exit_fades_to_the_warmed_flat_source() {
    let _serial = serial();
    for partition in [1, 257] {
        check_group_route(partition, RouteDirection::ExitLastChild, RouteEnd::Install);
    }
}

#[test]
fn cancelling_a_verified_last_child_exit_preserves_the_group_source() {
    let _serial = serial();
    check_group_route(257, RouteDirection::ExitLastChild, RouteEnd::Cancel);
}

#[test]
fn verified_last_child_exit_fades_the_old_group_and_discards_both_delay_histories() {
    let _serial = serial();
    check_group_route(
        257,
        RouteDirection::ExitLastChildWithDelay,
        RouteEnd::Install,
    );
    check_group_route(
        257,
        RouteDirection::ExitLastChildWithDelay,
        RouteEnd::Cancel,
    );
}

fn check_group_route(partition: usize, direction: RouteDirection, end: RouteEnd) {
    let predecessor_grouped = matches!(
        direction,
        RouteDirection::Exit
            | RouteDirection::ExitLastChild
            | RouteDirection::ExitLastChildWithDelay
    );
    let sibling_grouped = !matches!(
        direction,
        RouteDirection::EnterEmpty
            | RouteDirection::EnterEmptyWithDelay
            | RouteDirection::ExitLastChild
            | RouteDirection::ExitLastChildWithDelay
    );
    let stateful = matches!(
        direction,
        RouteDirection::EnterEmptyWithDelay | RouteDirection::ExitLastChildWithDelay
    );
    let check = |frame: u64, actual, gain, grouped, source_start| {
        let latency = if stateful {
            // Each Delay has a 1 ms buffer with no dry signal or feedback.
            if grouped { 96 } else { 48 }
        } else {
            0
        };
        let leaf = frame
            .checked_sub(latency)
            .filter(|frame| *frame >= source_start)
            .map_or(0.0, |frame| {
                f32::from(sample(u32::try_from(frame).unwrap())) / 32768.0
            })
            * gain;
        assert_frame(frame, actual, leaf, grouped, sibling_grouped);
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    let page = hex_engine::STREAM_PAGE_FRAMES as u64;
    let frames = u32::try_from(3 * page).unwrap();
    let cut = page - FADE as u64 / 2;
    let project = super::super::readiness::project_with_frames(u64::from(frames));
    let sibling = project.tracks()[0].id();
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_samples(frames, sample);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let mut editor = Editor::new(project, 8).unwrap();
    let selection = TrackSelectionIds::new(&[sibling]).unwrap();
    let Created::Tracks { first: leaf, .. } = editor
        .execute(Command::DuplicateTracks {
            tracks: selection.clone(),
            without_clips: false,
        })
        .unwrap()
    else {
        panic!("second leaf");
    };
    let Created::Track(group) = editor
        .execute(Command::GroupTracks {
            tracks: selection,
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group");
    };
    if !sibling_grouped {
        // Keep the populated fixture's other WAV audible as an unrelated root.
        // The empty Group still owns its Utility before receiving the leaf.
        editor
            .execute(Command::ReparentTracks {
                tracks: TrackSelectionIds::new(&[sibling]).unwrap(),
                destination: TrackDestination {
                    parent_group: None,
                    before: Some(group),
                },
            })
            .unwrap();
    }
    editor
        .execute(Command::Gain {
            track: sibling,
            gain: 0.25,
        })
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track: group,
            before: None,
            state: UTILITY_DEFINITION
                .patch_parameter(
                    &UTILITY_DEFINITION.default_state().unwrap(),
                    INVERT_LEFT,
                    ParameterValue::Bool(true),
                )
                .unwrap(),
        })
        .unwrap();
    if stateful {
        let state = hex_lattice_contract::NativeDeviceState::new(
            hex_devices::delay::TYPE_ID,
            1,
            [1.0f32, 0.0, 1.0]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect(),
        )
        .unwrap();
        for track in [group, leaf] {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: state.clone(),
                })
                .unwrap();
        }
    }
    let context = AudioSourceContext {
        project_generation: 2,
        editor_revision: 7,
    };
    let reparent = |editor: &mut Editor, parent_group| {
        editor
            .execute(Command::ReparentTracks {
                tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
                destination: TrackDestination {
                    parent_group,
                    before: None,
                },
            })
            .unwrap();
    };
    if predecessor_grouped {
        reparent(&mut editor, Some(group));
    }
    let rows = |editor: &Editor| {
        editor
            .project()
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>()
    };
    let expected_rows = if sibling_grouped {
        [group, sibling, leaf]
    } else {
        [sibling, group, leaf]
    };
    assert_eq!(rows(&editor), expected_rows);
    assert_eq!(
        editor.project().track(leaf).unwrap().parent_group(),
        predecessor_grouped.then_some(group)
    );
    let expected_sibling_parent = sibling_grouped.then_some(group);
    assert_eq!(
        editor.project().track(sibling).unwrap().parent_group(),
        expected_sibling_parent
    );
    let mut first =
        stream_fixture::owner_with_effects(editor.project(), metadata.clone(), Some(context));
    let next_parent = (!predecessor_grouped).then_some(group);
    reparent(&mut editor, next_parent);
    assert_eq!(rows(&editor), expected_rows);
    assert_eq!(
        editor.project().track(leaf).unwrap().parent_group(),
        next_parent
    );
    assert_eq!(
        editor.project().track(sibling).unwrap().parent_group(),
        expected_sibling_parent
    );
    let quote = AudioCandidateEffects::with_host_definitions(&[], |effects| {
        Ok(effects.quote(editor.project(), 1).unwrap())
    })
    .unwrap();
    let storage = quote.storage_reservation(0).unwrap();
    let mut limits = reservation(&mut first);
    limits.member_rows += quote.member_rows;
    limits.transfer_rows += quote.transfer_rows;
    limits.handle_rows += quote.handle_rows;
    limits.data_rows = limits.data_rows.max(quote.data_rows);
    limits.max_retained_data_bytes += storage.max_prepared_data_bytes;
    limits.max_construction_peak_data_bytes += storage.max_construction_data_bytes;
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(2).unwrap();
    let (stream_control, mut pages) = stream_fixture::pair_with_pool(vec![definition], 4);
    let warmup = Instant::now() + Duration::from_millis(400);
    for index in [0, 1] {
        pages.request_page(metadata.key(), index);
        while pages.page(metadata.key(), index).is_none() {
            pages.pump_ready(2);
            assert!(Instant::now() < warmup, "verified pages did not warm");
            std::thread::yield_now();
        }
    }
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            (stream_control, pages),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let first_ticket = control
        .stage_eligible_audio_candidate(
            first,
            vec![domain],
            PublicationPolicy::InheritRuntime,
            context.project_generation,
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    let predecessor = AppliedAudioSource {
        context,
        audio_revision: control.submitted_revision(),
        output_identity: control.output_identity().unwrap(),
    };
    let status = Arc::clone(&control.status);
    let mut render = |frames: usize| {
        let mut pcm = [[0.0; 2]; 257];
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut pcm[..frames]).unwrap();
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
        pcm
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    while status.applied_audio_source() != Some(predecessor) {
        render(1);
        assert!(Instant::now() < deadline, "predecessor did not install");
        std::thread::yield_now();
    }
    control.command(PlaybackCommand::Play).unwrap();
    while status.position.load(Acquire) < cut {
        let start = status.position.load(Acquire);
        let count = partition.min(usize::try_from(cut - start).unwrap());
        let pcm = render(count);
        if status.position.load(Acquire) != start {
            assert_eq!(status.position.load(Acquire), start + count as u64);
            for (offset, frame) in pcm[..count].iter().copied().enumerate() {
                check(start + offset as u64, frame, 1.0, predecessor_grouped, 0);
            }
        }
        assert!(Instant::now() < deadline, "predecessor did not play");
    }
    let edit_context = AudioSourceContext {
        editor_revision: context.editor_revision + 1,
        ..context
    };
    let candidate = stream_fixture::prepare_candidate(
        &mut control,
        editor.project(),
        &metadata,
        domain,
        edit_context,
    );
    let (ticket, receipt) = control
        .stage_verified_device_insert(
            candidate,
            edit_context,
            predecessor,
            None,
            Instant::now() + Duration::from_secs(2),
            |_| {},
        )
        .unwrap();
    let faded = match end {
        RouteEnd::Install => FADE,
        RouteEnd::Cancel => 32,
    };
    for start in (0..faded).step_by(partition) {
        let count = partition.min(faded - start);
        let pcm = render(count);
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        assert_eq!(status.applied_audio_source(), Some(predecessor));
        assert_eq!(status.position.load(Acquire), cut + (start + count) as u64);
        for (offset, frame) in pcm[..count].iter().copied().enumerate() {
            let elapsed = start + offset;
            check(
                cut + elapsed as u64,
                frame,
                (FADE - elapsed - 1) as f32 / FADE as f32,
                predecessor_grouped,
                0,
            );
        }
    }
    let (terminal, source, grouped) = match end {
        RouteEnd::Install => (
            PublicationReceiptState::Installed,
            AppliedAudioSource {
                context: edit_context,
                audio_revision: receipt.revision(),
                ..predecessor
            },
            !predecessor_grouped,
        ),
        RouteEnd::Cancel => {
            assert_eq!(
                receipt.cancel(),
                crate::prepared::FreshRuntimeCancellationResult::Cancelled
            );
            (
                PublicationReceiptState::Cancelled,
                predecessor,
                predecessor_grouped,
            )
        }
    };
    let starting_gain = (FADE - faded) as f32 / FADE as f32;
    let source_start = if stateful && matches!(end, RouteEnd::Install) {
        cut + faded as u64
    } else {
        0
    };
    for start in (0..=FADE).step_by(partition) {
        let count = partition.min(FADE + 1 - start);
        let pcm = render(count);
        assert_eq!(receipt.state(), terminal);
        assert_eq!(status.applied_audio_source(), Some(source));
        for (offset, frame) in pcm[..count].iter().copied().enumerate() {
            let elapsed = start + offset;
            let ramp = ((elapsed + 1) as f32 / FADE as f32).min(1.0);
            let gain = starting_gain + (1.0 - starting_gain) * ramp;
            check(
                cut + (faded + elapsed) as u64,
                frame,
                gain,
                grouped,
                source_start,
            );
        }
    }
    assert_eq!(
        status.position.load(Acquire),
        cut + (faded + FADE + 1) as u64
    );
    assert!(status.playing.load(Acquire));
    crate::lifecycle::test_dispose_backend(
        &mut Some(()),
        &mut false,
        &mut false,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    drop(recover(&mut control, &first_ticket));
    drop(recover(&mut control, &ticket));
    drop(control);
    finish(&mut retirement);
}
