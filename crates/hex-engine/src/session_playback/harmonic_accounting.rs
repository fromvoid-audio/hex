//! Read-only candidate accounting proof; no transition pool is installed.
use super::*;
use hex_project::{Command, Editor, Project};
use std::collections::BTreeMap;

#[path = "../../examples/harmonic_transition_layout.rs"]
mod layout;

fn inventory(effect_count: usize, held: bool, first_id: u64) -> Arc<PreparedRevision> {
    inventory_with_eq(effect_count, held, first_id, false)
}

fn inventory_with_eq(
    effect_count: usize,
    held: bool,
    first_id: u64,
    eq: bool,
) -> Arc<PreparedRevision> {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    editor
        .execute(Command::AddTrack {
            name: "layout".into(),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let template = snapshot.tracks.remove(0);
    let operator = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        hex_devices::operator_state::OPERATOR_TYPE_ID,
    )
    .unwrap();
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
    for _ in 0..hex_project::MAX_TRACKS {
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
                    id: next,
                    start_raw: 0,
                    length_raw: 8 << 32,
                    source_length_raw: 8 << 32,
                    source_start_raw: 0,
                    loop_start_raw: 0,
                    loop_end_raw: 8 << 32,
                    looping: false,
                    notes: vec![hex_project::MidiNoteSnapshot {
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
    let project = Project::from_snapshot(snapshot).unwrap();
    Arc::new(
        PreparedRevision::prepare(
            &project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

#[test]
fn candidate_pool_charge_with_maximum_track_and_effect_inventory() {
    let revision = inventory(hex_project::MAX_DEVICES_PER_TRACK, false, 100);
    let preflight = preroll::arrangement_runtime_storage_preflight_bytes(&revision).unwrap();
    let runtime = SessionPlayback::new(revision).unwrap();
    let actual = runtime.accounted_runtime_storage_bytes().unwrap();
    let pool = 2_639_456;
    eprintln!(
        "harmonic candidate inventory: tracks={} effects={} actual={} preflight={} pool={} combined_actual={} combined_preflight={} cap={}",
        hex_project::MAX_TRACKS,
        hex_project::MAX_TRACKS * hex_project::MAX_DEVICES_PER_TRACK,
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
        344
    );
}

#[test]
fn candidate_pool_accounting_includes_detached_mutable_tail_ledgers() {
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
    let proposed = existing.checked_add(2_639_456).unwrap();
    eprintln!(
        "harmonic retained-tail candidate: tails={tails} existing={existing} proposed={proposed} cap={} excess={}",
        MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
        proposed.saturating_sub(MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
    );
    assert_eq!(tails, hex_project::MAX_TRACKS);
    assert!(existing <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
    assert_eq!(existing, 29_880_296);
    assert_eq!(proposed, 32_519_752);
    assert!(proposed <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
}

#[test]
fn complete_pool_accounting_with_maximum_retained_memoryless_suffixes() {
    // Each real EQ chain retains 48 prefix and 16 suffix entries. The suffixes
    // exactly fill the unchanged 4096-entry processing budget; all 16384
    // allocated entries remain owned and charged across the handoff.
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
    let proposed = existing.checked_add(2_639_456).unwrap();
    eprintln!(
        "harmonic maximum-suffix candidate: tails={tails} suffixes={suffixes} existing={existing} proposed={proposed} cap={} excess={}",
        MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
        proposed.saturating_sub(MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
    );
    assert_eq!(tails, hex_project::MAX_TRACKS);
    assert_eq!(suffixes, MAX_RETAINED_SUFFIX_ENTRIES);
    assert_eq!(existing, 30_728_168);
    assert_eq!(proposed, 33_367_624);
    assert!(existing <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
    // Reconstruct the original c048995 negative control with identical
    // capacities/payload: only the old location's eight extra bytes differ.
    let legacy_proposed = proposed + 2 * SESSION_RELEASE_CAPACITY * 8;
    assert_eq!(legacy_proposed, 33_629_768);
    assert!(legacy_proposed > MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
    assert!(proposed <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
}

// Disconnected private planner and concrete proposed layouts, compiled only in tests.
mod admission_proof;
