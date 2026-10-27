//! Read-only candidate accounting proof; no transition pool is installed.
use super::*;
use hex_project::{Command, Editor, Project};
use std::collections::BTreeMap;

#[path = "../../examples/harmonic_transition_layout.rs"]
mod layout;

const ADMITTED_TRACKS: usize = 44;

fn inventory(effect_count: usize, held: bool, first_id: u64) -> Arc<PreparedRevision> {
    inventory_with_eq(effect_count, held, first_id, false)
}

fn inventory_with_eq(
    effect_count: usize,
    held: bool,
    first_id: u64,
    eq: bool,
) -> Arc<PreparedRevision> {
    inventory_shape(ADMITTED_TRACKS, effect_count, held, first_id, eq).unwrap()
}

fn inventory_shape(
    track_count: usize,
    effect_count: usize,
    held: bool,
    first_id: u64,
    eq: bool,
) -> Result<Arc<PreparedRevision>, crate::Invalid> {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    editor
        .execute(Command::InsertTrackWithDevice {
            name: "layout".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let template = snapshot.tracks.remove(0);
    // Synthetic inventory below assigns every source/device identity afresh.
    snapshot.native_sources.clear();
    let operator = crate::operator_legacy_test::legacy_operator_state();
    let operator = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &operator,
        hex_lattice_contract::ParameterId::from_static(82),
        hex_lattice_contract::ParameterValue::Bool(!eq),
    )
    .unwrap();
    let utility = crate::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let equalizer = crate::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.eq8"),
    )
    .unwrap();
    let equalizer = crate::patch_native_device_parameter(
        &equalizer,
        hex_devices::eq8::parameter::enabled(0, 0),
        hex_lattice_contract::ParameterValue::Bool(true),
    )
    .unwrap();
    let mut next = first_id;
    for _ in 0..track_count {
        let mut track = template.clone();
        track.id = next;
        next += 1;
        track.instrument.as_mut().unwrap().id = next;
        track.instrument.as_mut().unwrap().state = operator.clone();
        next += 1;
        if eq {
            track.instrument_position = Some(48);
        }
        for effect in 0..effect_count {
            track.devices.push(hex_project::DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: next,
                state: if eq && effect + 1 == effect_count {
                    equalizer.clone()
                } else {
                    utility.clone()
                },
                bypassed: false,
            });
            next += 1;
        }
        if held {
            track.content = hex_project::TrackContentSnapshot::Hybrid {
                audio_clips: Vec::new(),
                midi_clips: vec![hex_project::MidiClipSnapshot {
                    content_view: None,
                    active: true,
                    playback_offset_raw: 0,
                    one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                    volume: hex_project::MidiClipVolume::default(),
                    name: "clip".into(),
                    flavor: hex_project::ClipFlavor::default(),
                    id: next,
                    start_raw: 0,
                    length_raw: 8 << 32,
                    source_length_raw: 8 << 32,
                    source_start_raw: 0,
                    loop_start_raw: 0,
                    loop_end_raw: 8 << 32,
                    looping: false,
                    notes: vec![hex_project::MidiNoteSnapshot {
                        active: true,
                        id: next + 1,
                        start_raw: 0,
                        duration_raw: 8 << 32,
                        pitch: 60,
                        channel: 0,
                        velocity: 100,
                    }],
                }],
            };
            next += 2;
        }
        snapshot.tracks.push(track);
    }
    for _ in 0..hex_project::MAX_RETURN_TRACKS {
        let id = next;
        next += 1;
        let mut devices = Vec::new();
        for _ in 0..hex_project::MAX_DEVICES_PER_TRACK {
            devices.push(hex_project::DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: next,
                state: utility.clone(),
                bypassed: false,
            });
            next += 1;
        }
        snapshot
            .return_tracks
            .push(hex_project::ReturnTrackSnapshot {
                id,
                name: "layout return".into(),
                gain: 1.0,
                muted: false,
                devices,
            });
    }
    snapshot.next_id = next;
    let project = Project::from_legacy_snapshot(snapshot)?;
    Ok(Arc::new(PreparedRevision::prepare(
        &project,
        &BTreeMap::new(),
        Arc::new(hex_devices::SineTable::new()),
        None,
    )?))
}

#[test]
fn historical_full_project_shape_is_refused_by_current_signal_graph_bound() {
    let error = match inventory_shape(
        hex_project::MAX_TRACKS,
        hex_project::MAX_DEVICES_PER_TRACK,
        false,
        100,
        false,
    ) {
        Ok(_) => panic!("historical oversized graph unexpectedly prepared"),
        Err(error) => error,
    };
    assert_eq!(
        error,
        crate::Invalid("signal compensation graph exceeds its bounded node or edge limit")
    );
}

#[test]
fn candidate_pool_charge_with_admitted_track_and_effect_inventory() {
    // 44 full tracks plus 16 full Returns prepare to 4093 graph nodes:
    // 44 * (5 track nodes + 64 devices) + 16 * (2 Return nodes + 64 devices) + Main.
    let revision = inventory(hex_project::MAX_DEVICES_PER_TRACK, false, 100);
    let preflight = preroll::arrangement_runtime_storage_preflight_bytes(&revision).unwrap();
    let runtime = SessionPlayback::new(revision).unwrap();
    let actual = runtime.accounted_runtime_storage_bytes().unwrap();
    let pool = 2_643_560;
    eprintln!(
        "admitted harmonic candidate inventory: tracks={} effects={} actual={} preflight={} pool={} combined_actual={} combined_preflight={} cap={}",
        ADMITTED_TRACKS,
        ADMITTED_TRACKS * hex_project::MAX_DEVICES_PER_TRACK,
        actual,
        preflight,
        pool,
        actual + pool,
        preflight + pool,
        MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
    );
    assert!(actual <= preflight);
    assert!(preflight + pool <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
    assert_eq!(
        std::mem::size_of::<crate::midi_note_bank::PreparedMidiNote>(),
        352
    );
}

#[test]
fn admitted_inventory_accounting_includes_detached_mutable_tail_ledgers() {
    // Real existing publication, not artificial insertion into tail fields:
    // deleted old track IDs keep release owners while all new tracks have
    // their own constructor-allocated mutable runtime and effect inventory.
    let mut old = SessionPlayback::new(inventory(0, true, 100_000)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 96]).unwrap();
    let mut candidate = SessionPlayback::new(inventory(64, false, 100)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    let tails = candidate.tail_mutable_instruments.iter().flatten().count();
    assert_eq!(candidate.releases.capacity(), SESSION_RELEASE_CAPACITY);
    assert_eq!(
        candidate.release_scratch.capacity(),
        SESSION_RELEASE_CAPACITY
    );
    let existing = candidate.accounted_runtime_storage_bytes().unwrap();
    let proposed = existing.checked_add(2_643_560).unwrap();
    eprintln!(
        "harmonic retained-tail candidate: tails={tails} existing={existing} proposed={proposed} cap={} excess={}",
        MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
        proposed.saturating_sub(MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
    );
    assert_eq!(tails, ADMITTED_TRACKS);
    assert!(existing <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
    assert!(proposed <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
}

#[test]
fn admitted_inventory_accounting_charges_retained_memoryless_suffixes() {
    // Each admitted EQ chain retains 48 prefix and 16 suffix entries. The
    // independent planner test covers the 4096-entry aggregate suffix limit.
    let mut old = SessionPlayback::new(inventory_with_eq(64, true, 100_000, true)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 96]).unwrap();
    let mut candidate = SessionPlayback::new(inventory_with_eq(64, false, 100, true)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    let tails = candidate.tail_groups.iter().flatten().count();
    let suffixes: usize = candidate
        .tail_groups
        .iter()
        .enumerate()
        .filter_map(|(slot, chain)| {
            chain
                .as_ref()
                .map(|chain| chain.len() - candidate.tail_suffix_starts[slot])
        })
        .sum();
    assert_eq!(candidate.releases.capacity(), SESSION_RELEASE_CAPACITY);
    assert_eq!(
        candidate.release_scratch.capacity(),
        SESSION_RELEASE_CAPACITY
    );
    let existing = candidate.accounted_runtime_storage_bytes().unwrap();
    let proposed = existing.checked_add(2_643_560).unwrap();
    eprintln!(
        "admitted harmonic suffix candidate: tails={tails} suffixes={suffixes} existing={existing} proposed={proposed} cap={} excess={}",
        MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
        proposed.saturating_sub(MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
    );
    assert_eq!(tails, ADMITTED_TRACKS);
    assert_eq!(suffixes, ADMITTED_TRACKS * 16);
    assert!(existing <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
    assert!(proposed <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
}

// Disconnected private planner and concrete proposed layouts, compiled only in tests.
mod admission_proof;
