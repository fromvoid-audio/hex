//! Resident Group route cuts with independent continuous Delay oracles.
#![cfg(feature = "allocation-probe")]
use hex_audio::prepared::{self, PublicationPolicy, PublicationReceiptState};
use hex_engine::{
    Audio, Frame, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState, ParameterValue};
use hex_project::{
    Command, Created, Editor, Project, TrackDestination, TrackId, TrackSelectionIds,
};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering::Acquire},
};

const RATE: u32 = 48_000;
const FRAMES: usize = 6144;
const CUT: usize = 2048;
const FADE: usize = RATE.div_ceil(200) as usize;
type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn delay(ms: f32) -> NativeDeviceState {
    use hex_devices::delay::{
        DEFINITION, DRY_WET, FEEDBACK, FILTER, HIGH_QUALITY, LEFT_SYNC, TIME,
    };
    let mut state = DEFINITION.default_state().unwrap();
    for parameter in [LEFT_SYNC, FILTER, HIGH_QUALITY] {
        state = DEFINITION
            .patch_parameter(&state, parameter, ParameterValue::Bool(false))
            .unwrap();
    }
    for (parameter, value) in [(TIME, ms), (FEEDBACK, 0.5), (DRY_WET, 1.0)] {
        state = DEFINITION
            .patch_parameter(&state, parameter, ParameterValue::Float(value))
            .unwrap();
    }
    state
}
fn invert() -> NativeDeviceState {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter::INVERT_LEFT};
    UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap()
}
fn import(
    editor: &mut Editor,
    media: &mut BTreeMap<hex_project::AssetId, Audio>,
    name: &str,
    pcm: Vec<Frame>,
) -> Result<TrackId> {
    let Created::Imported { track, asset, .. } = editor.execute(Command::ImportAudioTrack {
        name: name.into(),
        path: format!("memory/{name}.wav").into(),
        sample_rate: RATE,
        frames: pcm.len() as u64,
    })?
    else {
        return Err("audio fixture".into());
    };
    media.insert(asset, Audio::new(RATE, pcm)?);
    Ok(track)
}
fn effect(editor: &mut Editor, track: TrackId, state: NativeDeviceState) -> Result {
    editor.execute(Command::InsertDevice {
        track,
        before: None,
        state,
    })?;
    Ok(())
}
fn group(editor: &mut Editor, track: TrackId, name: &str) -> Result<TrackId> {
    let Created::Track(group) = editor.execute(Command::GroupTracks {
        tracks: TrackSelectionIds::new(&[track])?,
        name: name.into(),
    })?
    else {
        return Err("group fixture".into());
    };
    Ok(group)
}
fn prepare(
    editor: &Editor,
    media: &BTreeMap<hex_project::AssetId, Audio>,
) -> Result<Arc<PreparedRevision>> {
    Ok(Arc::new(PreparedRevision::prepare(
        editor.project(),
        media,
        Arc::new(SineTable::new()),
        None,
    )?))
}
/// Each oracle is one uninterrupted, independently prepared single-chain run.
/// It never inherits an edited graph or uses the route-transfer implementation.
fn standalone(
    pcm: Vec<Frame>,
    track_chain: &[NativeDeviceState],
    main_chain: &[NativeDeviceState],
) -> Result<Vec<Frame>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let mut media = BTreeMap::new();
    let track = import(&mut editor, &mut media, "oracle", pcm)?;
    for state in track_chain {
        effect(&mut editor, track, state.clone())?;
    }
    for state in main_chain {
        editor.execute(Command::InsertMainDevice {
            before: None,
            state: state.clone(),
        })?;
    }
    let mut playback = Playback::try_from_prepared_with_session(prepare(&editor, &media)?)?;
    playback.apply(PlaybackCommand::Play);
    let mut output = vec![[0.0; 2]; FRAMES];
    playback.render(&mut output)?;
    Ok(output)
}
fn measured(callback: &mut prepared::Callback, output: &mut [Frame]) {
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(output).unwrap();
    });
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
#[derive(Clone, Copy, Debug)]
enum Terminal {
    Install,
    Cancel,
    Stop,
}

#[derive(Clone, Copy)]
enum Reroute {
    Enter,
    Exit,
    EnterEmpty,
    ExitLastChild,
    EnterSoleGroup,
    ExitSoleGroup,
    EnterNested,
    ExitNested,
    EnterFromAbove,
    ExitAbove,
    BetweenGroups,
    EnterWithMainDelay,
}

#[test]
fn allocation_probe_detects_balanced_allocation_and_free() {
    let observed = allocation_counter::measure(|| {
        let value = std::hint::black_box(Box::new(std::hint::black_box(42_u64)));
        std::hint::black_box(&value);
        drop(value);
    });
    assert_eq!((observed.count_total, observed.bytes_total), (1, 8));
    assert_eq!((observed.count_current, observed.bytes_current), (0, 0));
}

#[test]
fn group_entry_cancel_and_stop_preserve_the_old_route_before_and_during_fade() -> Result {
    for partition in [1, 257] {
        for terminal in [Terminal::Cancel, Terminal::Stop] {
            for fade in [false, true] {
                route_cut(partition, terminal, fade, Reroute::Enter)?;
            }
        }
    }
    Ok(())
}
#[test]
fn group_entry_fades_only_leaf_at_single_frame_partitions() -> Result {
    route_cut(1, Terminal::Install, true, Reroute::Enter)
}
#[test]
fn group_entry_fades_only_leaf_at_257_frame_partitions() -> Result {
    route_cut(257, Terminal::Install, true, Reroute::Enter)
}
#[test]
fn group_exit_cancel_and_stop_preserve_the_old_group_before_and_during_fade() -> Result {
    for partition in [1, 257] {
        for terminal in [Terminal::Cancel, Terminal::Stop] {
            for fade in [false, true] {
                route_cut(partition, terminal, fade, Reroute::Exit)?;
            }
        }
    }
    Ok(())
}

#[test]
fn group_exit_fades_only_leaf_at_single_frame_partitions() -> Result {
    route_cut(1, Terminal::Install, true, Reroute::Exit)
}
#[test]
fn group_exit_fades_only_leaf_at_257_frame_partitions() -> Result {
    route_cut(257, Terminal::Install, true, Reroute::Exit)
}

#[test]
fn group_reroutes_across_the_first_or_last_child() -> Result {
    for partition in [1, 257] {
        for reroute in [Reroute::EnterEmpty, Reroute::ExitLastChild] {
            route_cut(partition, Terminal::Install, true, reroute)?;
        }
    }
    Ok(())
}

#[test]
fn abandoning_a_last_child_exit_restores_the_former_group_feed() -> Result {
    for terminal in [Terminal::Cancel, Terminal::Stop] {
        route_cut(257, terminal, true, Reroute::ExitLastChild)?;
    }
    Ok(())
}

#[test]
fn moving_the_first_or_last_child_of_the_only_group_preserves_history() -> Result {
    for partition in [1, 257] {
        for reroute in [Reroute::EnterSoleGroup, Reroute::ExitSoleGroup] {
            route_cut(partition, Terminal::Install, true, reroute)?;
        }
    }
    Ok(())
}

#[test]
fn abandoning_a_reroute_across_the_only_group_preserves_the_old_route() -> Result {
    for reroute in [Reroute::EnterSoleGroup, Reroute::ExitSoleGroup] {
        for terminal in [Terminal::Cancel, Terminal::Stop] {
            route_cut(257, terminal, true, reroute)?;
        }
    }
    Ok(())
}

#[test]
fn nested_group_reroutes_preserve_both_ancestors() -> Result {
    for partition in [1, 257] {
        for reroute in [Reroute::EnterNested, Reroute::ExitNested] {
            route_cut(partition, Terminal::Install, true, reroute)?;
        }
    }
    Ok(())
}

#[test]
fn abandoning_a_nested_group_reroute_preserves_its_predecessor() -> Result {
    for reroute in [Reroute::EnterNested, Reroute::ExitNested] {
        for terminal in [Terminal::Cancel, Terminal::Stop] {
            route_cut(257, terminal, true, reroute)?;
        }
    }
    Ok(())
}

#[test]
fn changing_the_moving_leafs_row_keeps_the_correct_fader_and_history() -> Result {
    for partition in [1, 257] {
        for reroute in [Reroute::EnterFromAbove, Reroute::ExitAbove] {
            route_cut(partition, Terminal::Install, true, reroute)?;
        }
    }
    Ok(())
}

#[test]
fn moving_between_groups_keeps_both_groups_running() -> Result {
    for partition in [1, 257] {
        route_cut(partition, Terminal::Install, true, Reroute::BetweenGroups)?;
    }
    route_cut(257, Terminal::Cancel, true, Reroute::BetweenGroups)
}

#[test]
fn group_reparent_preserves_the_existing_main_delay() -> Result {
    for partition in [1, 257] {
        route_cut(
            partition,
            Terminal::Install,
            true,
            Reroute::EnterWithMainDelay,
        )?;
    }
    for terminal in [Terminal::Cancel, Terminal::Stop] {
        route_cut(257, terminal, true, Reroute::EnterWithMainDelay)?;
    }
    Ok(())
}

fn route_cut(partition: usize, terminal: Terminal, fade: bool, reroute: Reroute) -> Result {
    let exiting = matches!(
        reroute,
        Reroute::Exit
            | Reroute::ExitLastChild
            | Reroute::ExitSoleGroup
            | Reroute::ExitNested
            | Reroute::ExitAbove
    );
    let moves_from_above = matches!(reroute, Reroute::EnterFromAbove | Reroute::ExitAbove);
    let between_groups = matches!(reroute, Reroute::BetweenGroups);
    let changes_row_order = moves_from_above || between_groups;
    let has_main_delay = matches!(reroute, Reroute::EnterWithMainDelay);
    let nested = matches!(reroute, Reroute::EnterNested | Reroute::ExitNested);
    let has_other_group = !matches!(reroute, Reroute::EnterSoleGroup | Reroute::ExitSoleGroup);
    let has_sibling = changes_row_order
        || nested
        || has_main_delay
        || matches!(reroute, Reroute::Enter | Reroute::Exit);
    let leaf_background = if has_main_delay {
        [0.0; 2]
    } else {
        [0.125, -0.0625]
    };
    let mut a = vec![leaf_background; FRAMES];
    a[1700] = [0.5, -0.25];
    a[CUT + 53] = [-0.375, 0.1875];
    let b: Vec<Frame> = if has_sibling {
        (0..FRAMES)
            .map(|i| [0.03125 + i as f32 / 262144.0, -0.015625])
            .collect()
    } else {
        vec![[0.0; 2]; FRAMES]
    };
    let c: Vec<Frame> = (0..FRAMES)
        .map(|i| [-0.015625, 0.03125 + i as f32 / 524288.0])
        .collect();
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let mut media = BTreeMap::new();
    let sibling = has_sibling
        .then(|| import(&mut editor, &mut media, "destination child", b.clone()))
        .transpose()?;
    let moved = import(&mut editor, &mut media, "moved", a.clone())?;
    let destination = group(&mut editor, sibling.unwrap_or(moved), "destination")?;
    if sibling.is_none() {
        editor.execute(Command::ReparentTracks {
            tracks: TrackSelectionIds::new(&[moved])?,
            destination: TrackDestination {
                parent_group: None,
                before: None,
            },
        })?;
        assert!(editor.project().track(destination).unwrap().is_group());
    }
    effect(&mut editor, destination, delay(11.0))?;
    effect(&mut editor, destination, invert())?;
    if nested {
        let outer = group(&mut editor, destination, "outer group")?;
        effect(&mut editor, outer, delay(3.0))?;
    }
    effect(&mut editor, moved, delay(7.0))?;
    let unrelated = import(&mut editor, &mut media, "unrelated", c.clone())?;
    let unrelated_root = if has_other_group {
        group(&mut editor, unrelated, "other group")?
    } else {
        unrelated
    };
    effect(&mut editor, unrelated_root, delay(13.0))?;
    let mut main_chain = vec![delay(5.0)];
    if has_main_delay {
        // This is the only positive-delay stage and exists before either cut.
        main_chain.push(hex_devices::limiter::DEFINITION.default_state().unwrap());
    }
    for state in &main_chain {
        editor.execute(Command::InsertMainDevice {
            before: None,
            state: state.clone(),
        })?;
    }
    if between_groups {
        editor.execute(Command::ReparentTracks {
            tracks: TrackSelectionIds::new(&[moved])?,
            destination: TrackDestination {
                parent_group: Some(unrelated_root),
                before: None,
            },
        })?;
    } else if moves_from_above {
        editor.execute(Command::MoveTracksBefore {
            tracks: TrackSelectionIds::new(&[moved])?,
            before: Some(destination),
        })?;
    }
    let before = prepare(&editor, &media)?;
    let inventory: Vec<_> = editor.project().tracks().iter().map(|t| t.id()).collect();
    editor.execute(Command::ReparentTracks {
        tracks: TrackSelectionIds::new(&[moved])?,
        destination: TrackDestination {
            parent_group: Some(destination),
            before: None,
        },
    })?;
    let entered_inventory: Vec<_> = editor.project().tracks().iter().map(|t| t.id()).collect();
    assert_eq!(entered_inventory != inventory, changes_row_order);
    let after = prepare(&editor, &media)?;
    let (before, after) = if exiting {
        editor.execute(Command::ReparentTracks {
            tracks: TrackSelectionIds::new(&[moved])?,
            destination: TrackDestination {
                parent_group: None,
                before: Some(if changes_row_order {
                    destination
                } else {
                    unrelated_root
                }),
            },
        })?;
        assert_eq!(
            editor
                .project()
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect::<Vec<_>>(),
            inventory
        );
        (after, prepare(&editor, &media)?)
    } else {
        (before, after)
    };
    let main_latency = before.arrangement().main_latency_frames();
    assert_eq!(main_latency > 0, has_main_delay);
    assert_eq!(after.arrangement().main_latency_frames(), main_latency);

    let mut leaf = standalone(a, &[delay(7.0)], &[])?;
    let fade_frames = if matches!(terminal, Terminal::Install) {
        FADE
    } else {
        32
    };
    let switch = CUT + if fade { fade_frames } else { 0 };
    if fade {
        // The existing child Delay keeps running; only its routed output fades.
        // Destination/independent Groups and Main keep their own uninterrupted DSP.
        for (i, frame) in leaf.iter_mut().enumerate().skip(CUT) {
            let gain = if i < switch {
                (CUT + FADE - i - 1) as f32 / FADE as f32
            } else {
                let start = (FADE - fade_frames) as f32 / FADE as f32;
                start + (1.0 - start) * ((i - switch + 1) as f32 / FADE as f32).min(1.0)
            };
            for sample in frame {
                *sample *= gain;
            }
        }
    }
    // The former Group keeps its history. The leaf's output has one route
    // per frame and its own chain stays running across the cut.
    let grouped_at = |i| {
        let installed = matches!(terminal, Terminal::Install) && i >= switch;
        if exiting { !installed } else { installed }
    };
    let mut destination_input = b;
    let mut other_group_input = c;
    for i in 0..FRAMES {
        if grouped_at(i) {
            for channel in 0..2 {
                destination_input[i][channel] += leaf[i][channel];
            }
        } else if between_groups {
            for channel in 0..2 {
                other_group_input[i][channel] += leaf[i][channel];
            }
        }
    }
    let other_pcm = standalone(other_group_input, &[delay(13.0)], &[])?;
    let destination_pcm = standalone(destination_input, &[delay(11.0), invert()], &[])?;
    let destination_pcm = if nested {
        standalone(destination_pcm, &[delay(3.0)], &[])?
    } else {
        destination_pcm
    };
    let mut mixed = vec![[0.0; 2]; FRAMES];
    for i in 0..FRAMES {
        for channel in 0..2 {
            mixed[i][channel] = destination_pcm[i][channel] + other_pcm[i][channel];
            if !between_groups && !grouped_at(i) {
                mixed[i][channel] += leaf[i][channel];
            }
        }
    }
    let expected = if has_main_delay {
        standalone(mixed, &[], &main_chain)?
    } else {
        standalone(mixed, &main_chain, &[])?
    };
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control.configure_device_resource_publication_on_control(
        hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: 100_000_000,
            max_construction_peak_bytes: 200_000_000,
        },
    )?;
    let context = hex_engine::AudioSourceContext {
        project_generation: 1,
        editor_revision: 0,
    };
    control
        .publish_with_admission_and_source_on_control(
            Playback::try_from_prepared_with_session(before)?,
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .map_err(|_| "initial publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut actual = vec![[0.0; 2]; FRAMES];
    for output in actual[..CUT].chunks_mut(partition) {
        measured(&mut callback, output);
    }
    let predecessor = control.status.recording_source().ok_or("source")?;
    let fence = control.capture_fresh_runtime_fence().ok_or("fence")?;
    let revision = control
        .publish_with_admission_and_source_on_control(
            Playback::try_from_prepared_with_session(after)?,
            PublicationPolicy::DeviceInsert { predecessor, fence },
            hex_engine::AudioSourceContext {
                editor_revision: 1,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .map_err(|_| "route publication")?;
    let receipt = control.fresh_runtime_receipt(revision).ok_or("receipt")?;
    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    assert_eq!(control.status.position.load(Acquire), CUT as u64);
    assert_eq!(control.status.recording_source(), Some(predecessor));
    for output in actual[CUT..switch].chunks_mut(partition) {
        measured(&mut callback, output);
        assert_eq!(
            receipt.state(),
            PublicationReceiptState::Pending,
            "old parent must own the entire fade-down"
        );
        assert_eq!(control.status.recording_source(), Some(predecessor));
    }
    match terminal {
        Terminal::Cancel => {
            receipt.cancel();
        }
        Terminal::Stop => control.stop(),
        Terminal::Install => {}
    }
    for output in actual[switch..].chunks_mut(partition) {
        measured(&mut callback, output);
        if matches!(terminal, Terminal::Install) {
            assert_eq!(receipt.state(), PublicationReceiptState::Installed);
            assert_eq!(
                control.status.recording_source().unwrap().audio_revision,
                revision
            );
        }
    }
    match terminal {
        Terminal::Install => {
            assert_eq!(receipt.state(), PublicationReceiptState::Installed);
            assert_eq!(
                control.status.recording_source().unwrap().audio_revision,
                revision
            );
        }
        Terminal::Cancel => {
            assert_eq!(receipt.state(), PublicationReceiptState::Cancelled);
            assert_eq!(control.status.recording_source(), Some(predecessor));
        }
        Terminal::Stop => {
            assert_eq!(
                receipt.state(),
                PublicationReceiptState::Refused(
                    hex_engine::PlaybackHandoffRejection::RuntimeFenceChanged
                )
            );
            assert!(actual[switch..].iter().flatten().all(|v| *v == 0.0));
        }
    }
    if !matches!(terminal, Terminal::Stop) {
        assert_eq!(control.status.position.load(Acquire), FRAMES as u64);
        assert!(control.status.playing.load(Acquire));
    }
    let compared = if matches!(terminal, Terminal::Stop) {
        switch
    } else {
        FRAMES
    };
    for (i, (actual, expected)) in actual[..compared].iter().zip(&expected).enumerate() {
        for channel in 0..2 {
            assert!(
                (actual[channel] - expected[channel]).abs() < 2e-6,
                "{terminal:?} partition {partition}, frame {i}, channel {channel}: {} != {}",
                actual[channel],
                expected[channel]
            );
        }
    }
    if matches!(terminal, Terminal::Stop) {
        assert_eq!(control.status.position.load(Acquire), 0);
        assert!(!control.status.playing.load(Acquire));
    }
    control.reclaim();
    Ok(())
}
