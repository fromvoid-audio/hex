use super::{
    payload,
    planner::{Input, OwnerCharge, Recipe, Root},
};
use crate::mutable_instrument::{MutableInstrumentTail, MutableInstrumentTrackRuntime};
use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
use hex_lattice_contract::{DeviceTypeId, ParameterValue};
use hex_project::{Command, Editor, Project};
use std::{collections::BTreeMap, mem::size_of, sync::Arc};

#[derive(Clone, Copy, Debug)]
pub(super) enum Instrument {
    Analytic,
    Lfo,
    Feedback,
    Sine,
    Fm4,
}
pub(super) struct Shape {
    pub tracks: usize,
    pub effect: &'static str,
    pub effect_tracks: usize,
    pub returns: usize,
    pub mutable: Option<(usize, Instrument)>,
    pub instrument: Instrument,
}

fn operator(kind: Instrument) -> hex_lattice_contract::NativeDeviceState {
    let type_id = match kind {
        Instrument::Sine => hex_devices::SINE_INSTRUMENT_TYPE_ID,
        Instrument::Fm4 => DeviceTypeId::from_static("hex.instrument.fm4"),
        _ => hex_devices::operator_state::OPERATOR_TYPE_ID,
    };
    let state = if matches!(kind, Instrument::Sine | Instrument::Fm4) {
        crate::default_native_device_state_for_class(crate::NativeDeviceClass::Instrument, type_id)
            .unwrap()
    } else {
        crate::operator_legacy_test::legacy_operator_state()
    };
    match kind {
        Instrument::Lfo => crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Instrument,
            &state,
            hex_lattice_contract::ParameterId::from_static(82),
            ParameterValue::Bool(true),
        )
        .unwrap(),
        Instrument::Feedback => crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Instrument,
            &state,
            hex_devices::operator_state::parameter::feedback(1),
            ParameterValue::Float(0.2),
        )
        .unwrap(),
        _ => state,
    }
}

fn effect(id: &'static str) -> hex_lattice_contract::NativeDeviceState {
    let state = crate::default_native_device_state(DeviceTypeId::from_static(id)).unwrap();
    match id {
        "hex.eq8" => {
            let enabled = crate::patch_native_device_parameter(
                &state,
                hex_devices::eq8::parameter::enabled(0, 0),
                ParameterValue::Bool(true),
            )
            .unwrap();
            crate::patch_native_device_parameter(
                &enabled,
                hex_devices::eq8::parameter::gain(0, 0),
                ParameterValue::Float(6.0),
            )
            .unwrap()
        }
        "hex.utility" => crate::patch_native_device_parameter(
            &state,
            hex_devices::utility::parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap(),
        _ => state,
    }
}

pub(super) fn revision(shape: &Shape, held: bool, first_id: u64) -> Arc<PreparedRevision> {
    assert!(shape.effect_tracks <= shape.tracks);
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    editor
        .execute(Command::InsertTrackWithDevice {
            name: "admission proof".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let template = snapshot.tracks.remove(0);
    // Synthetic inventory below assigns every source/device identity afresh.
    snapshot.native_sources.clear();
    let utility =
        crate::default_native_device_state(DeviceTypeId::from_static("hex.utility")).unwrap();
    let stateful = effect(shape.effect);
    let mut next = first_id;
    for index in 0..shape.tracks {
        let mut track = template.clone();
        track.id = next;
        next += 1;
        let kind = shape
            .mutable
            .filter(|(count, _)| index < *count)
            .map_or(shape.instrument, |(_, kind)| kind);
        track.instrument.as_mut().unwrap().id = next;
        next += 1;
        track.instrument.as_mut().unwrap().state = operator(kind);
        track.instrument_position = Some(48);
        for slot in 0..64 {
            track.devices.push(hex_project::DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: next,
                state: if index >= shape.tracks - shape.effect_tracks && slot == 63 {
                    stateful.clone()
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
    for index in 0..16 {
        let id = next;
        next += 1;
        let mut devices = Vec::new();
        for slot in 0..64 {
            devices.push(hex_project::DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: next,
                state: if index < shape.returns && slot == 63 {
                    stateful.clone()
                } else {
                    utility.clone()
                },
                bypassed: false,
            });
            next += 1;
        }
        snapshot
            .return_tracks
            .push(hex_project::ReturnTrackSnapshot {
                id,
                name: "proof return".into(),
                gain: 1.0,
                muted: false,
                devices,
            });
    }
    // Real sounding Return history, rather than silent allocated Return slots.
    for bus in snapshot.return_tracks.iter().take(shape.returns) {
        snapshot.tracks[0]
            .sends
            .push(hex_project::SendLevelSnapshot {
                return_track: bus.id,
                gain: 0.1,
            });
    }
    snapshot.next_id = next;
    let project = Project::from_legacy_snapshot(snapshot).unwrap();
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

pub(super) fn handoff(shape: &Shape) -> Result<SessionPlayback, crate::SessionPlaybackError> {
    handoff_with(shape, revision(shape, true, 100_000), || {
        revision(shape, false, 100)
    })
}

pub(super) fn handoff_prepared(
    shape: &Shape,
    previous_revision: Arc<PreparedRevision>,
    candidate_revision: Arc<PreparedRevision>,
) -> Result<SessionPlayback, crate::SessionPlaybackError> {
    handoff_with(shape, previous_revision, || candidate_revision)
}

fn handoff_with(
    shape: &Shape,
    previous_revision: Arc<PreparedRevision>,
    candidate_revision: impl FnOnce() -> Arc<PreparedRevision>,
) -> Result<SessionPlayback, crate::SessionPlaybackError> {
    let mut previous = SessionPlayback::new(previous_revision).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    previous.render(&mut [[0.0; 2]; 96]).unwrap();
    let mut candidate = SessionPlayback::new(candidate_revision()).unwrap();
    if let Err(error) = candidate.try_inherit_state_from(&mut previous) {
        eprintln!(
            "current handoff refusal: effects={} returns={} mutable={:?} track_pair_bytes={} return_pair_bytes={} instrument_pair_bytes={} error={error:?}",
            shape.effect_tracks,
            shape.returns,
            shape.mutable,
            previous
                .device_chains
                .iter()
                .map(|chain| chain.declared_pair_bytes())
                .sum::<usize>(),
            previous
                .return_chains
                .iter()
                .map(|chain| chain.declared_pair_bytes())
                .sum::<usize>(),
            previous
                .mutable_instruments
                .iter()
                .flatten()
                .map(|owner| owner.declared_pair_bytes())
                .sum::<usize>(),
        );
        return Err(error);
    }
    Ok(candidate)
}

pub(super) struct Ownership {
    pub base: usize,
    pub active: Vec<OwnerCharge>,
    pub retained: Vec<OwnerCharge>,
    pub contexts: Vec<usize>,
    pub retained_contexts: Vec<usize>,
    pub active_effects: usize,
    pub retained_effects: usize,
    pub active_effect_bytes: usize,
    pub retained_effect_bytes: usize,
    pub suffixes: usize,
}
impl Ownership {
    pub fn input<'a>(&'a self, recipes: &'a [Recipe], pool: payload::PoolCapacity) -> Input<'a> {
        Input {
            base: self.base,
            roots: &[Root {
                owner: 1,
                next_generation: u64::MAX,
            }],
            staged_bindings: self.contexts.len(),
            active: &self.active,
            retained: &self.retained,
            current_contexts: &self.contexts,
            retained_contexts: &self.retained_contexts,
            recipes,
            pool,
            context_capacity: payload::ContextCapacity::allocated(),
            active_effects: self.active_effects,
            retained_effects: self.retained_effects,
            active_effect_bytes: self.active_effect_bytes,
            retained_effect_bytes: self.retained_effect_bytes,
            suffix_entries: self.suffixes,
        }
    }
}

pub(super) fn ownership(runtime: &SessionPlayback, proposed: bool) -> Ownership {
    let mut base = runtime.accounted_runtime_storage_bytes().unwrap();
    let mut collect = |owner: &MutableInstrumentTrackRuntime| {
        base = base
            .checked_sub(owner.accounted_storage_bytes().unwrap())
            .unwrap();
        let source_live_bytes = if owner.is_source_owner() {
            MutableInstrumentTrackRuntime::source_live_storage_upper_bound()
        } else {
            0
        };
        let ledgers = owner
            .ledger_capacity_bytes()
            .unwrap()
            .checked_add(source_live_bytes)
            .unwrap();
        assert_eq!(
            ledgers,
            128 * size_of::<crate::mutable_instrument::MutableOccurrence>() + source_live_bytes
        );
        OwnerCharge {
            header: if proposed {
                size_of::<payload::HostOwner>()
            } else {
                size_of::<MutableInstrumentTrackRuntime>()
            },
            payload: if proposed {
                size_of::<payload::Pair>()
            } else {
                owner.runtime_pair_storage_bytes().unwrap()
            },
            ledgers,
        }
    };
    let active = runtime
        .mutable_instruments
        .iter()
        .flatten()
        .map(&mut collect)
        .collect();
    let retained = runtime
        .tail_mutable_instruments
        .iter()
        .flatten()
        .map(|tail| collect(&tail.runtime))
        .collect();
    if proposed {
        // Preserve the original duplicate owner-header accounting policy.
        base = base
            .checked_sub(
                runtime.mutable_instruments.capacity()
                    * size_of::<Option<MutableInstrumentTrackRuntime>>(),
            )
            .unwrap()
            .checked_add(
                runtime.mutable_instruments.capacity() * size_of::<Option<payload::HostOwner>>(),
            )
            .unwrap();
        base = base
            .checked_sub(
                runtime.tail_mutable_instruments.capacity()
                    * size_of::<Option<MutableInstrumentTail>>(),
            )
            .unwrap()
            .checked_add(
                runtime.tail_mutable_instruments.capacity()
                    * size_of::<Option<payload::TailOwner>>(),
            )
            .unwrap();
    }
    let table = runtime.revision.session().shared_contexts();
    let contexts = (0..256)
        .filter_map(|slot| {
            table.owner(slot).map(|owner| {
                if proposed {
                    payload::context_bytes()
                } else {
                    owner.declared_bytes()
                }
            })
        })
        .collect();
    let retained_contexts = runtime
        .retained_contexts
        .iter()
        .flatten()
        .map(|owner| {
            if proposed {
                payload::context_bytes()
            } else {
                owner.declared_bytes()
            }
        })
        .collect();
    let active_chains = runtime.device_chains.iter().chain(&runtime.return_chains);
    let active_effects = active_chains
        .clone()
        .map(|chain| chain.stateful_count())
        .sum();
    let active_effect_bytes = active_chains.map(|chain| chain.declared_pair_bytes()).sum();
    let retained_chains = runtime.tail_groups.iter().flatten().chain(
        runtime
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .map(|tail| &tail.chain),
    );
    let retained_effects = retained_chains
        .clone()
        .map(|chain| chain.stateful_count())
        .sum();
    let retained_effect_bytes = retained_chains
        .map(|chain| chain.declared_pair_bytes())
        .sum();
    let suffixes = runtime
        .tail_groups
        .iter()
        .enumerate()
        .filter_map(|(index, chain)| {
            chain
                .as_ref()
                .map(|chain| chain.len() - runtime.tail_suffix_starts[index])
        })
        .sum::<usize>()
        + runtime
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .map(|tail| tail.chain.len())
            .sum::<usize>();
    Ownership {
        base,
        active,
        retained,
        contexts,
        retained_contexts,
        active_effects,
        retained_effects,
        active_effect_bytes,
        retained_effect_bytes,
        suffixes,
    }
}
