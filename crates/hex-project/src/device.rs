//! Canonical device identity and provider state; preparation stays outside the model.
//!
//! # Serial host-unit extension contract
//!
//! This is the selected extension boundary, not a claim that nested devices are
//! processed. `Device` and `DeviceSnapshot` retain bounded serial membership;
//! the engine's
//! `compile::prepare_source_instruments` also refuses multi-device sources.
//! Parallel Audio Rack branches and reusable modulation remain separate owners.
//!
//! A serial unit is one existing `Device` followed by its contiguous child units
//! in the canonical owner chain. Each node stores its stable parent identity;
//! bounded preorder validation avoids a second registry and preserves existing
//! identity, automation, capacity and provider-state codecs. Keep the suffix in
//! the canonical model, not in opaque provider payloads, a UI tree, or a second Rack registry. Signal order is
//! processor then children in preorder, followed by the next sibling. This does
//! not introduce parallel branches, another instrument position or MIDI routing.
//! Session/engine provider admission permits compatible effects as children;
//! additional generators and unsupported provider resources must refuse.
//!
//! Each authored node has one globally unique `DeviceId`. Moves within an owner
//! preserve the entire subtree and identities; copies reserve fresh identities
//! and remap internal references atomically. Reject self/descendant insertion,
//! duplicate/missing identities, stale anchors and unsupported owner transfers.
//! A retained-source definition is counted once, never once per track binding.
//! Its immutable binding views must still be replaced together by
//! `Project::edit_device` / `edit_source_device`, not mutated independently.
//!
//! Bound depth to eight nodes including the root. Count every descendant against
//! the existing owning-chain and global device budgets: 64 local/Main/Return
//! effect nodes, or 65 nodes in a source including its designated instrument.
//! Do not raise those budgets by nesting. Validate depth, counts, identity ranges
//! and checked payload/resource quotes before allocation or partial mutation.
//! Provider-internal Rack resources retain their own admission/accounting; they
//! are not extra free capacity hidden in a serial node.
//!
//! Existing state/bypass/comparison/automation queries, clipboard, snapshot and
//! edit paths must share a bounded canonical traversal/location contract. Extend
//! `DeviceChainOwner` only with real supported owners/child destinations. Do not
//! make unrelated callers decode a provider payload to locate a child. A first
//! model/storage increment must include an engine refusal gate for unsupported
//! nonempty units; silently preparing only the parent is never an intermediate
//! implementation. Allocate a fresh storage schema at implementation time,
//! preserve earlier flat snapshots as empty suffixes, and refuse future/malformed
//! trees without rewriting source files. The current storage ceiling is owned by
//! `hex-storage`, not by this module or an assumed next schema number.
//!
//! Authored `bypassed` remains local to each node. Effective bypass includes all
//! ancestors: an effect unit passes its dry input around the entire suffix; an
//! instrument unit produces no source output. Child switches/settings remain
//! authored and recover when the ancestor is enabled. Flattened preparation may
//! reuse existing serial processors and bypass machinery, but must propagate
//! that effective state, PDC, stable parameter targets and resource custody.
//! Preparation/publication is all-or-nothing through the existing session,
//! engine and audio candidate owners; callbacks acquire no new tree ownership.
//!
//! NativeSource processing is a distinct runtime increment. The singleton-only
//! binding and preparation guards must not be relaxed merely because children
//! are now representable. The complete source unit must run once for its merged
//! MIDI contributors and deliver audio only to its explicit receiver; local track
//! processing stays separate. Cover Arrangement and Session together before
//! admitting a bound instrument's nonempty suffix.
//!
//! Portable presets capture the authored unit: provider states, each node's
//! A/B bank, host gains, authored power, children and supported host macros,
//! modulators and mappings. Song automation, source/track bindings, receiver,
//! transport and live DSP instances are not portable unit state. Internal targets
//! use relative identities remapped on insertion; unavailable external mappings
//! refuse rather than silently retarget. Asset references in every provider/A/B
//! variant need retained media capture and destination remapping, reusing the
//! clipboard/resource owners, not temporary original-project IDs or file paths.
//!
//! The existing preset schemas 1/2 stay state-only operations: loading one into
//! an existing unit must retain its children, host gains and power as today's
//! state replacement does. Fresh legacy insertion creates an ordinary empty
//! suffix. A new full-unit envelope uses a new schema and explicit structural
//! replacement, preserving the target root identity and allocating new child
//! identities in one prepared history transaction. Reuse DeviceInitialization
//! and worker preset/default publication; do not apply children piecemeal or
//! claim future modulation/provider capabilities from an empty serialized stub.
use crate::{Command, DeviceId, Invalid, Project, TrackId};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};

#[cfg(test)]
#[path = "device_unit_removal_tests.rs"]
mod unit_removal_tests;

pub const MAX_DEVICES_PER_TRACK: usize = 64;
pub const MAX_DEVICES: usize = crate::MAX_TRACKS * (MAX_DEVICES_PER_TRACK + 1)
    + crate::MAX_RETURN_TRACKS * MAX_DEVICES_PER_TRACK;
pub const DEFAULT_INSTRUMENT_TYPE_ID: DeviceTypeId =
    DeviceTypeId::from_static("hex.instrument.sine");

/// Neutral envelope for explicit Sine insertion and existing legacy-schema readers.
/// Project/storage do not depend on the DSP definition or its implementation.
pub fn default_instrument_state() -> Result<NativeDeviceState, Invalid> {
    NativeDeviceState::new(DEFAULT_INSTRUMENT_TYPE_ID, 1, Vec::new())
        .map_err(|_| Invalid("could not construct default instrument state"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub(crate) parent: Option<DeviceId>,
    pub(crate) automation: Vec<crate::DeviceAutomation>,
    pub(crate) id: DeviceId,
    pub(crate) state: NativeDeviceState,
    pub(crate) bypassed: bool,
    pub(crate) comparison: Option<crate::DeviceComparison>,
}
impl Device {
    /// Serial-unit parent in this canonical chain; `None` identifies a root.
    pub fn parent(&self) -> Option<DeviceId> {
        self.parent
    }
    pub fn id(&self) -> DeviceId {
        self.id
    }
    pub fn state(&self) -> &NativeDeviceState {
        &self.state
    }
    pub fn bypassed(&self) -> bool {
        self.bypassed
    }
}

impl Project {
    /// Control-side, history-free preview of an existing device-state edit.
    /// Reuse the canonical command implementation on an isolated metadata copy;
    /// this grants no media/runtime/source ownership or identity reservation.
    /// Commands that change routing, identity or other model owners refuse.
    pub fn preview_device_edit(&self, command: &Command) -> Result<Self, Invalid> {
        if !matches!(
            command,
            Command::ReplaceDeviceState { .. } | Command::ManualDeviceEdit { .. }
        ) {
            return Err(Invalid("command has no device-state preview"));
        }
        let mut candidate = self.clone();
        candidate.apply(command.clone())?;
        Ok(candidate)
    }
}

pub(crate) fn apply(project: &mut Project, command: Command) -> Result<crate::Created, Invalid> {
    match command {
        Command::InsertDevice {
            track,
            before,
            state,
        } => {
            let id = insert(project, track, before, state)?;
            Ok(crate::Created::Device(id))
        }
        Command::InsertInstrument { track, state } => {
            let id = insert_instrument(project, track, state)?;
            Ok(crate::Created::Device(id))
        }
        Command::RemoveInstrument { track } => {
            if project
                .track(track)
                .ok_or(Invalid("track no longer exists"))?
                .instrument
                .is_none()
            {
                return Err(Invalid("instrument slot is already empty"));
            }
            project.bind_track_source(track, None)?;
            Ok(crate::Created::None)
        }
        Command::RemoveDevice(id) => {
            if let Ok((track, index)) = locate_device(project, id) {
                let span = crate::device_inventory::serial_unit_span(
                    &project.tracks[track].devices,
                    index,
                )?;
                let removed = span.len();
                drop(project.tracks[track].devices.drain(span));
                if project.tracks[track]
                    .instrument_position
                    .is_some_and(|position| index < position)
                {
                    project.tracks[track].instrument_position = project.tracks[track]
                        .instrument_position
                        .and_then(|position| position.checked_sub(removed));
                }
            } else if let Some((track, index)) =
                project
                    .returns
                    .iter()
                    .enumerate()
                    .find_map(|(track, item)| {
                        item.devices
                            .iter()
                            .position(|device| device.id == id)
                            .map(|index| (track, index))
                    })
            {
                let span = crate::device_inventory::serial_unit_span(
                    &project.returns[track].devices,
                    index,
                )?;
                drop(project.returns[track].devices.drain(span));
            } else if let Some(index) = project
                .main_devices
                .iter()
                .position(|device| device.id == id)
            {
                let span = crate::device_inventory::serial_unit_span(&project.main_devices, index)?;
                drop(project.main_devices.drain(span));
            } else {
                return Err(Invalid("device no longer exists"));
            }
            Ok(crate::Created::None)
        }
        Command::MoveDevice {
            device,
            track,
            before,
        } => {
            move_device(project, device, track, before)?;
            Ok(crate::Created::None)
        }
        Command::MoveTrackDevice {
            device,
            track,
            before,
        } => {
            move_track_device(project, device, track, before)?;
            Ok(crate::Created::None)
        }
        Command::ReplaceDeviceState { device, state } => {
            project.edit_device(device, |d| d.replace_state(state))?;
            Ok(crate::Created::None)
        }
        Command::ReplaceDeviceStateWithIdentityReservation {
            automation,
            device,
            expected_state,
            expected_variant,
            state,
            expected_next_id,
            reserved_ids,
        } => {
            if project.next_id != expected_next_id
                || project
                    .device(device)
                    .is_none_or(|entry| entry.active_variant() != expected_variant)
                || project
                    .device(device)
                    .ok_or(Invalid("device no longer exists"))?
                    .state()
                    != &expected_state
            {
                return Err(Invalid(
                    "device changed; retry the edit from its current state",
                ));
            }
            let next = expected_next_id
                .checked_add(reserved_ids)
                .ok_or(Invalid("project identity space exhausted"))?;
            project.edit_device(device, |d| match automation {
                Some(change) => d.replace_state_and_automation(state, change),
                None => d.replace_state(state),
            })?;
            project.next_id = next;
            Ok(crate::Created::None)
        }
        Command::CompareDevice {
            device,
            expected,
            action,
        } => {
            project.edit_device(device, |d| d.compare(expected, action))?;
            Ok(crate::Created::None)
        }
        Command::ReplaceDeviceComparison {
            device,
            state,
            comparison,
        } => {
            if let Some(value) = &comparison {
                value.validate(&state)?;
            }
            project.edit_device(device, |target| {
                target.validate_automation_provider(&state)?;
                target.state = state;
                target.comparison = comparison;
                Ok(())
            })?;
            Ok(crate::Created::None)
        }
        Command::SetDeviceBypass { device, bypassed } => {
            project.edit_device(device, |d| {
                d.bypassed = bypassed;
                Ok(())
            })?;
            Ok(crate::Created::None)
        }
        Command::ToggleDeviceBypass { device } => {
            project.edit_device(device, |d| {
                d.bypassed = !d.bypassed;
                Ok(())
            })?;
            Ok(crate::Created::None)
        }
        _ => Err(Invalid("command is not a device edit")),
    }
}

fn insert_instrument(
    project: &mut Project,
    track: TrackId,
    state: NativeDeviceState,
) -> Result<DeviceId, Invalid> {
    let target = project
        .tracks
        .iter()
        .find(|candidate| candidate.id == track)
        .ok_or(Invalid("track no longer exists"))?;
    if target.instrument.is_some() {
        return Err(Invalid("instrument slot is already occupied"));
    }
    let total_devices = crate::returns::device_count(project)?;
    if total_devices >= MAX_DEVICES {
        return Err(Invalid("project device limit reached"));
    }
    let name = target.name.clone();
    let id = DeviceId(project.allocate_id()?);
    let binding = project.register_instrument(
        name,
        Device {
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id,
            state,
            bypassed: false,
        },
        track,
    )?;
    project.track_mut(track)?.instrument = Some(binding);
    project.track_mut(track)?.instrument_position = Some(0);
    Ok(id)
}

fn insert(
    project: &mut Project,
    track: TrackId,
    before: Option<DeviceId>,
    state: NativeDeviceState,
) -> Result<DeviceId, Invalid> {
    let target = project
        .tracks
        .iter()
        .find(|candidate| candidate.id == track)
        .ok_or(Invalid("track no longer exists"))?;
    if target.devices.len() >= MAX_DEVICES_PER_TRACK {
        return Err(Invalid("track device limit reached"));
    }
    let total_devices = crate::returns::device_count(project)?;
    if total_devices >= MAX_DEVICES {
        return Err(Invalid("project device limit reached"));
    }
    let before_instrument = before.is_some_and(|id| {
        target
            .instrument
            .as_ref()
            .is_some_and(|instrument| instrument.id == id)
    });
    let index = if before_instrument {
        target
            .instrument_position
            .ok_or(Invalid("instrument position is missing"))?
    } else {
        anchor_index(&target.devices, before)?
    };
    let id = DeviceId(project.allocate_id()?);
    let target = project
        .tracks
        .iter_mut()
        .find(|candidate| candidate.id == track)
        .ok_or(Invalid("track no longer exists"))?;
    target.devices.insert(
        index,
        Device {
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id,
            state,
            bypassed: false,
        },
    );
    if before_instrument
        || target
            .instrument_position
            .is_some_and(|position| index < position)
    {
        target.instrument_position = Some(
            target
                .instrument_position
                .and_then(|position| position.checked_add(1))
                .ok_or(Invalid("instrument position overflow"))?,
        );
    }
    Ok(id)
}

fn locate_device(project: &Project, id: DeviceId) -> Result<(usize, usize), Invalid> {
    project
        .tracks
        .iter()
        .enumerate()
        .find_map(|(track_index, track)| {
            track
                .devices
                .iter()
                .position(|device| device.id == id)
                .map(|index| (track_index, index))
        })
        .ok_or(Invalid("device no longer exists"))
}
fn anchor_index(devices: &[Device], before: Option<DeviceId>) -> Result<usize, Invalid> {
    match before {
        Some(id) => devices
            .iter()
            .position(|device| device.id == id)
            .ok_or(Invalid("device insertion anchor no longer exists")),
        None => Ok(devices.len()),
    }
}
fn move_device(
    project: &mut Project,
    id: DeviceId,
    target_track: TrackId,
    before: Option<DeviceId>,
) -> Result<(), Invalid> {
    let (source_index, source_position) = locate_device(project, id)?;
    let target_index = project
        .tracks
        .iter()
        .position(|track| track.id == target_track)
        .ok_or(Invalid("target track no longer exists"))?;
    if before == Some(id) && source_index == target_index {
        return Ok(());
    }
    let target_len_after = project.tracks[target_index]
        .devices
        .len()
        .saturating_sub(usize::from(source_index == target_index));
    if source_index != target_index && target_len_after >= MAX_DEVICES_PER_TRACK {
        return Err(Invalid("track device limit reached"));
    }
    if source_index == target_index {
        let devices = &project.tracks[source_index].devices;
        let destination = match before {
            Some(anchor) => {
                let anchor_position = devices
                    .iter()
                    .position(|device| device.id == anchor)
                    .ok_or(Invalid("device insertion anchor no longer exists"))?;
                if anchor_position == source_position {
                    return Err(Invalid("device cannot be moved before itself"));
                }
                anchor_position - usize::from(anchor_position > source_position)
            }
            None => devices
                .len()
                .checked_sub(1)
                .ok_or(Invalid("device count underflow"))?,
        };
        let old_gap = project.tracks[source_index].instrument_position;
        let gap_after_remove = old_gap.map(|gap| if source_position < gap { gap - 1 } else { gap });
        let gap_after_insert =
            gap_after_remove.map(|gap| if destination < gap { gap + 1 } else { gap });
        let moving = project.tracks[source_index].devices.remove(source_position);
        project.tracks[source_index]
            .devices
            .insert(destination, moving);
        project.tracks[source_index].instrument_position = gap_after_insert;
    } else {
        let destination = anchor_index(&project.tracks[target_index].devices, before)?;
        let old_gap = project.tracks[source_index].instrument_position;
        let moving = project.tracks[source_index].devices.remove(source_position);
        if old_gap.is_some_and(|gap| source_position < gap) {
            project.tracks[source_index].instrument_position =
                old_gap.and_then(|gap| gap.checked_sub(1));
        }
        project.tracks[target_index]
            .devices
            .insert(destination, moving);
        if project.tracks[target_index]
            .instrument_position
            .is_some_and(|gap| destination < gap)
        {
            let gap = project.tracks[target_index]
                .instrument_position
                .and_then(|gap| gap.checked_add(1))
                .ok_or(Invalid("instrument position overflow"))?;
            project.tracks[target_index].instrument_position = Some(gap);
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum DeviceLocation {
    Effect { track: usize, index: usize },
    Instrument { track: usize },
}

fn locate_chain_device(project: &Project, id: DeviceId) -> Result<DeviceLocation, Invalid> {
    for (track_index, track) in project.tracks.iter().enumerate() {
        if let Some(index) = track.devices.iter().position(|device| device.id == id) {
            return Ok(DeviceLocation::Effect {
                track: track_index,
                index,
            });
        }
        if track
            .instrument
            .as_ref()
            .is_some_and(|device| device.id == id)
        {
            return Ok(DeviceLocation::Instrument { track: track_index });
        }
    }
    Err(Invalid("device no longer exists"))
}

fn chain_effect_index_before(
    track: &crate::Track,
    before: Option<DeviceId>,
) -> Result<usize, Invalid> {
    match before {
        Some(id) => {
            if track
                .instrument
                .as_ref()
                .is_some_and(|device| device.id == id)
            {
                return track
                    .instrument_position
                    .ok_or(Invalid("instrument position is missing"));
            }
            track
                .devices
                .iter()
                .position(|device| device.id == id)
                .ok_or(Invalid("device insertion anchor no longer exists"))
        }
        None => Ok(track.devices.len()),
    }
}

fn move_track_device(
    project: &mut Project,
    id: DeviceId,
    target_track: TrackId,
    before: Option<DeviceId>,
) -> Result<(), Invalid> {
    let source = locate_chain_device(project, id)?;
    let target_index = project
        .tracks
        .iter()
        .position(|track| track.id == target_track)
        .ok_or(Invalid("target track no longer exists"))?;
    let source_index = match source {
        DeviceLocation::Effect { track, .. } | DeviceLocation::Instrument { track } => track,
    };
    if before == Some(id) && source_index == target_index {
        return Ok(());
    }
    if let Some(anchor) = before {
        let anchor_location = locate_chain_device(project, anchor)?;
        let anchor_track = match anchor_location {
            DeviceLocation::Effect { track, .. } | DeviceLocation::Instrument { track } => track,
        };
        if anchor_track != target_index {
            return Err(Invalid("device insertion anchor belongs to another track"));
        }
    }
    if matches!(source, DeviceLocation::Instrument { .. })
        && source_index != target_index
        && project.tracks[target_index].instrument.is_some()
    {
        return Err(Invalid("target track already has an instrument"));
    }
    let target_effects_after = project.tracks[target_index]
        .devices
        .len()
        .checked_sub(usize::from(
            source_index == target_index && matches!(source, DeviceLocation::Effect { .. }),
        ))
        .ok_or(Invalid("device count underflow"))?;
    if matches!(source, DeviceLocation::Effect { .. })
        && source_index != target_index
        && target_effects_after >= MAX_DEVICES_PER_TRACK
    {
        return Err(Invalid("track device limit reached"));
    }

    let receiver_transfer = match source {
        DeviceLocation::Instrument { track } => {
            let source_track = project.tracks[track].id;
            project.tracks[track].source_id().filter(|source| {
                project
                    .native_source(*source)
                    .is_some_and(|definition| definition.receiver() == Some(source_track))
            })
        }
        DeviceLocation::Effect { .. } => None,
    };

    let moving = match source {
        DeviceLocation::Effect { track, index } => {
            let old_position = project.tracks[track].instrument_position;
            let device = project.tracks[track].devices.remove(index);
            if old_position.is_some_and(|position| index < position) {
                project.tracks[track].instrument_position =
                    old_position.and_then(|p| p.checked_sub(1));
            }
            (Some(device), None)
        }
        DeviceLocation::Instrument { track } => {
            let device = project.tracks[track]
                .instrument
                .take()
                .ok_or(Invalid("instrument slot is already empty"))?;
            project.tracks[track].instrument_position = None;
            (None, Some(device))
        }
    };
    {
        let target = &mut project.tracks[target_index];
        if let Some(binding) = moving.1 {
            let position = chain_effect_index_before(target, before)?;
            target.instrument = Some(binding);
            target.instrument_position = Some(position);
        } else {
            let position = chain_effect_index_before(target, before)?;
            let before_instrument = before.is_some_and(|anchor| {
                target
                    .instrument
                    .as_ref()
                    .is_some_and(|instrument| instrument.id == anchor)
            });
            let device = moving.0.ok_or(Invalid("moving effect is missing"))?;
            target.devices.insert(position, device);
            if before_instrument
                || target
                    .instrument_position
                    .is_some_and(|instrument_position| position < instrument_position)
            {
                target.instrument_position = Some(
                    target
                        .instrument_position
                        .and_then(|instrument_position| instrument_position.checked_add(1))
                        .ok_or(Invalid("instrument position overflow"))?,
                );
            }
        }
    }
    if let Some(source) = receiver_transfer {
        project.set_native_source_receiver(source, Some(target_track))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::DeviceTypeId;

    fn state(payload: Vec<u8>) -> NativeDeviceState {
        NativeDeviceState::new(DeviceTypeId::new("test.device").unwrap(), 1, payload).unwrap()
    }
    fn track_id(editor: &mut crate::Editor, name: &str) -> TrackId {
        match editor
            .execute(Command::InsertTrackWithDevice {
                name: name.into(),
                before_track: None,
                slot: crate::TrackDeviceSlot::Instrument,
                state: crate::default_instrument_state().unwrap(),
            })
            .unwrap()
        {
            crate::Created::Track(id) => id,
            other => panic!("unexpected edit result: {other:?}"),
        }
    }
    fn device_id(editor: &mut crate::Editor, track: TrackId) -> DeviceId {
        match editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: state(vec![7]),
            })
            .unwrap()
        {
            crate::Created::Device(id) => id,
            other => panic!("unexpected edit result: {other:?}"),
        }
    }

    #[test]
    fn device_preview_matches_canonical_edit_without_history_or_identity_changes() {
        let mut editor = crate::Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let track = track_id(&mut editor, "preview");
        let device = device_id(&mut editor, track);
        let original = editor.project().clone();
        let command = Command::ReplaceDeviceState {
            device,
            state: state(vec![4, 2]),
        };
        let preview = original.preview_device_edit(&command).unwrap();
        assert_eq!(editor.project(), &original);
        editor.execute(command).unwrap();
        assert_eq!(editor.project(), &preview);
        assert!(editor.undo());
        assert_eq!(editor.project(), &original);
        assert!(
            original
                .preview_device_edit(&Command::InsertDevice {
                    track,
                    before: None,
                    state: state(vec![]),
                })
                .is_err()
        );
        assert!(
            original
                .preview_device_edit(&Command::Gain { track, gain: 0.25 })
                .is_err()
        );
        let mut missing = original.clone();
        missing.apply(Command::RemoveDevice(device)).unwrap();
        assert!(
            missing
                .preview_device_edit(&Command::ReplaceDeviceState {
                    device,
                    state: state(vec![]),
                })
                .is_err()
        );
        assert_eq!(editor.project(), &original);
    }

    #[test]
    fn opaque_identity_reservation_slot_switch_refuses_before_prepare() {
        let mut editor = crate::Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let track = track_id(&mut editor, "track");
        let device = device_id(&mut editor, track);
        let command = Command::ReplaceDeviceStateWithIdentityReservation {
            automation: None,
            device,
            expected_state: editor.project().device(device).unwrap().state().clone(),
            expected_variant: crate::DeviceVariant::A,
            state: state(vec![9]),
            expected_next_id: editor.project().next_identity(),
            reserved_ids: 1,
        };
        editor
            .execute(Command::CompareDevice {
                device,
                expected: crate::DeviceVariant::A,
                action: crate::DeviceComparisonAction::Switch,
            })
            .unwrap();
        let before = editor.project().snapshot();
        let calls = std::cell::Cell::new(0);
        let result = editor.execute_prepared(command, |_| {
            calls.set(calls.get() + 1);
            Ok::<(), Invalid>(())
        });
        assert!(
            result.is_err(),
            "an A-origin edit must not retarget identical B"
        );
        assert_eq!(calls.get(), 0, "refuse before preparation");
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.undo());
        assert_eq!(
            editor.project().device(device).unwrap().active_variant(),
            crate::DeviceVariant::A
        );
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), before);
        // A fresh B-owned capture remains a valid identity-reserving edit.
        let entry = editor.project().device(device).unwrap();
        editor
            .execute(Command::ReplaceDeviceStateWithIdentityReservation {
                automation: None,
                device,
                expected_state: entry.state().clone(),
                expected_variant: entry.active_variant(),
                state: state(vec![9]),
                expected_next_id: editor.project().next_identity(),
                reserved_ids: 1,
            })
            .unwrap();
        assert_eq!(
            editor.project().device(device).unwrap().active_variant(),
            crate::DeviceVariant::B
        );
        assert_eq!(
            editor.project().device(device).unwrap().state(),
            &state(vec![9])
        );
        assert!(editor.undo());
        assert_eq!(
            editor.project().device(device).unwrap().state(),
            &state(vec![7])
        );
    }

    #[test]
    fn opaque_identity_reservation_rejects_stale_edits_and_undo_never_reuses_ids() {
        let mut editor = crate::Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let track = track_id(&mut editor, "track");
        let device = device_id(&mut editor, track);
        let expected = editor.project().device(device).unwrap().state().clone();
        let first = editor.project().next_identity();
        let command = Command::ReplaceDeviceStateWithIdentityReservation {
            automation: None,
            device,
            expected_state: expected.clone(),
            expected_variant: crate::DeviceVariant::A,
            state: state(vec![9]),
            expected_next_id: first,
            reserved_ids: 41,
        };
        editor.execute(command.clone()).unwrap();
        assert_eq!(editor.project().next_identity(), first + 41);
        assert!(editor.execute(command).is_err());
        assert_eq!(
            editor.project().device(device).unwrap().state(),
            &state(vec![9])
        );
        assert!(editor.undo());
        assert_eq!(editor.project().device(device).unwrap().state(), &expected);
        assert_eq!(editor.project().next_identity(), first + 41);
        let snapshot = editor.project().snapshot();
        assert!(
            editor
                .execute(Command::ReplaceDeviceStateWithIdentityReservation {
                    automation: None,
                    device,
                    expected_state: expected,
                    expected_variant: crate::DeviceVariant::A,
                    state: state(vec![10]),
                    expected_next_id: first + 41,
                    reserved_ids: u64::MAX
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), snapshot);
    }

    #[test]
    fn generic_device_commands_allocate_stable_ids_and_order() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let track = track_id(&mut editor, "track");
        let first = device_id(&mut editor, track);
        let second = match editor
            .execute(Command::InsertDevice {
                track,
                before: Some(first),
                state: state(vec![8]),
            })
            .unwrap()
        {
            crate::Created::Device(id) => id,
            _ => panic!("expected device"),
        };
        editor
            .execute(Command::ReplaceDeviceState {
                device: first,
                state: state(vec![9]),
            })
            .unwrap();
        assert_eq!(
            editor
                .project()
                .track(track)
                .unwrap()
                .devices()
                .iter()
                .map(Device::id)
                .collect::<Vec<_>>(),
            [second, first]
        );
        assert_eq!(
            editor.project().device(first).unwrap().state().payload(),
            [9]
        );
        assert!(editor.undo());
        assert_eq!(
            editor.project().device(first).unwrap().state().payload(),
            [7]
        );
        assert!(editor.redo());
        assert_eq!(
            editor.project().device(first).unwrap().state().payload(),
            [9]
        );
    }

    #[test]
    fn device_move_bypass_removal_and_failed_edits_are_atomic() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let first_track = track_id(&mut editor, "first");
        let second_track = track_id(&mut editor, "second");
        let first = device_id(&mut editor, first_track);
        let second = device_id(&mut editor, first_track);
        editor
            .execute(Command::MoveDevice {
                device: first,
                track: second_track,
                before: None,
            })
            .unwrap();
        editor
            .execute(Command::SetDeviceBypass {
                device: first,
                bypassed: true,
            })
            .unwrap();
        assert!(editor.project().device(first).unwrap().bypassed());
        assert_eq!(
            editor.project().track(first_track).unwrap().devices()[0].id(),
            second
        );
        assert_eq!(
            editor.project().track(second_track).unwrap().devices()[0].id(),
            first
        );
        let before = editor.project().clone();
        assert!(
            editor
                .execute(Command::MoveDevice {
                    device: first,
                    track: first_track,
                    before: Some(DeviceId(u64::MAX))
                })
                .is_err()
        );
        assert!(
            editor
                .execute(Command::InsertDevice {
                    track: first_track,
                    before: Some(DeviceId(u64::MAX)),
                    state: state(vec![10])
                })
                .is_err()
        );
        assert_eq!(*editor.project(), before);
        editor.execute(Command::RemoveDevice(first)).unwrap();
        assert!(editor.project().device(first).is_none());
        assert!(editor.undo());
        assert!(editor.project().device(first).is_some());
    }

    #[test]
    fn legacy_move_preflights_same_track_anchor_before_mutating_project() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let track = track_id(&mut editor, "legacy move");
        let first = device_id(&mut editor, track);
        let second = device_id(&mut editor, track);
        let mut candidate = editor.project().clone();
        let before = candidate.clone();

        assert!(move_device(&mut candidate, second, track, Some(DeviceId(u64::MAX))).is_err());
        assert_eq!(candidate, before);
        assert!(move_device(&mut candidate, second, track, Some(second)).is_ok());
        assert_eq!(candidate, before);
        assert_eq!(candidate.track(track).unwrap().devices()[0].id(), first);
    }

    #[test]
    fn canonical_chain_moves_effects_and_instrument_across_the_persisted_gap() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let track = track_id(&mut editor, "chain");
        let instrument = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let first = device_id(&mut editor, track);
        let second = device_id(&mut editor, track);
        let move_before = |editor: &mut crate::Editor, device, before| {
            editor
                .execute(Command::MoveTrackDevice {
                    device,
                    track,
                    before,
                })
                .unwrap();
        };
        let chain_ids = |editor: &crate::Editor| {
            editor
                .project()
                .track(track)
                .unwrap()
                .ordered_device_chain()
                .map(|node| match node {
                    crate::TrackDeviceRef::Instrument(device)
                    | crate::TrackDeviceRef::Effect(device) => device.id(),
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(chain_ids(&editor), [instrument, first, second]);
        move_before(&mut editor, instrument, Some(second));
        assert_eq!(chain_ids(&editor), [first, instrument, second]);
        assert_eq!(
            editor.project().track(track).unwrap().instrument_position(),
            Some(1)
        );
        move_before(&mut editor, second, Some(instrument));
        assert_eq!(chain_ids(&editor), [first, second, instrument]);
        move_before(&mut editor, first, None);
        assert_eq!(chain_ids(&editor), [second, instrument, first]);
        assert_eq!(
            editor.project().track(track).unwrap().instrument_position(),
            Some(1)
        );
        move_before(&mut editor, instrument, Some(second));
        assert_eq!(chain_ids(&editor), [instrument, second, first]);
        move_before(&mut editor, first, Some(instrument));
        assert_eq!(chain_ids(&editor), [first, instrument, second]);

        let before_noop = editor.project().snapshot();
        let can_undo_before_noop = editor.can_undo();
        move_before(&mut editor, first, Some(instrument));
        assert_eq!(editor.project().snapshot(), before_noop);
        assert_eq!(editor.can_undo(), can_undo_before_noop);
        assert!(editor.undo());
        assert_eq!(chain_ids(&editor), [instrument, second, first]);

        let destination = track_id(&mut editor, "destination");
        let destination_instrument = editor
            .project()
            .track(destination)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        editor
            .execute(Command::MoveTrackDevice {
                device: first,
                track: destination,
                before: Some(destination_instrument),
            })
            .unwrap();
        assert_eq!(
            editor
                .project()
                .track(destination)
                .unwrap()
                .instrument_position(),
            Some(1)
        );
        assert!(
            editor
                .execute(Command::MoveTrackDevice {
                    device: instrument,
                    track: destination,
                    before: None,
                })
                .is_err()
        );
        editor
            .execute(Command::RemoveInstrument { track: destination })
            .unwrap();
        let destination_effect = device_id(&mut editor, destination);
        editor
            .execute(Command::MoveTrackDevice {
                device: instrument,
                track: destination,
                before: Some(first),
            })
            .unwrap();
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );
        assert_eq!(
            editor
                .project()
                .track(destination)
                .unwrap()
                .instrument_position(),
            Some(0)
        );
        assert_eq!(
            editor
                .project()
                .track(destination)
                .unwrap()
                .ordered_device_chain()
                .map(|node| match node {
                    crate::TrackDeviceRef::Instrument(device)
                    | crate::TrackDeviceRef::Effect(device) => device.id(),
                })
                .collect::<Vec<_>>(),
            [instrument, first, destination_effect]
        );
        assert!(
            editor.project().device(destination_instrument).is_some(),
            "unlink retains the original source device"
        );
        assert!(
            editor
                .project()
                .native_sources()
                .iter()
                .any(|source| source.instrument().id() == destination_instrument)
        );
        let before_failure = editor.project().snapshot();
        assert!(
            editor
                .execute(Command::MoveTrackDevice {
                    device: first,
                    track: destination,
                    before: Some(DeviceId(u64::MAX)),
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before_failure);
    }

    #[test]
    fn effect_insert_remove_and_instrument_recreate_keep_gap_consistent() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let track = track_id(&mut editor, "gap edits");
        let instrument = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let first = match editor
            .execute(Command::InsertDevice {
                track,
                before: Some(instrument),
                state: state(vec![1]),
            })
            .unwrap()
        {
            crate::Created::Device(id) => id,
            _ => panic!("expected first effect"),
        };
        let second = match editor
            .execute(Command::InsertDevice {
                track,
                before: Some(instrument),
                state: state(vec![2]),
            })
            .unwrap()
        {
            crate::Created::Device(id) => id,
            _ => panic!("expected second effect"),
        };
        let track_view = editor.project().track(track).unwrap();
        assert_eq!(track_view.instrument_position(), Some(2));
        assert_eq!(
            track_view
                .ordered_device_chain()
                .map(|node| match node {
                    crate::TrackDeviceRef::Instrument(device)
                    | crate::TrackDeviceRef::Effect(device) => device.id(),
                })
                .collect::<Vec<_>>(),
            [first, second, instrument]
        );
        editor.execute(Command::RemoveDevice(first)).unwrap();
        assert_eq!(
            editor.project().track(track).unwrap().instrument_position(),
            Some(1)
        );
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        assert_eq!(
            editor.project().track(track).unwrap().instrument_position(),
            None
        );
        let new_instrument = match editor
            .execute(Command::InsertInstrument {
                track,
                state: crate::default_instrument_state().unwrap(),
            })
            .unwrap()
        {
            crate::Created::Device(id) => id,
            _ => panic!("expected instrument"),
        };
        assert_eq!(
            editor.project().track(track).unwrap().instrument_position(),
            Some(0)
        );
        assert_eq!(
            editor.project().track(track).unwrap().devices()[0].id(),
            second
        );
        assert_ne!(new_instrument, instrument);
    }

    #[test]
    fn device_limit_and_allocator_high_water_are_enforced() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 128).unwrap();
        let track = track_id(&mut editor, "track");
        for _ in 0..MAX_DEVICES_PER_TRACK {
            device_id(&mut editor, track);
        }
        let before = editor.project().clone();
        assert!(
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: state(vec![])
                })
                .is_err()
        );
        assert_eq!(*editor.project(), before);
        let mut snapshot = editor.project().snapshot();
        snapshot.next_id = u64::MAX;
        let mut exhausted =
            crate::Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
        let before = exhausted.project().clone();
        assert!(
            exhausted
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: state(vec![])
                })
                .is_err()
        );
        assert_eq!(*exhausted.project(), before);
    }

    #[test]
    fn instrument_slot_state_bypass_remove_insert_and_undo_are_atomic() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let track = track_id(&mut editor, "keys");
        let original = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        assert_eq!(
            editor.project().device(original).unwrap().state(),
            &crate::default_instrument_state().unwrap()
        );

        let custom = state(vec![3, 9, 1]);
        editor
            .execute(Command::ReplaceDeviceState {
                device: original,
                state: custom.clone(),
            })
            .unwrap();
        editor
            .execute(Command::SetDeviceBypass {
                device: original,
                bypassed: true,
            })
            .unwrap();
        assert!(editor.project().device(original).unwrap().bypassed());
        assert_eq!(editor.project().device(original).unwrap().state(), &custom);
        assert!(editor.undo());
        assert!(!editor.project().device(original).unwrap().bypassed());
        assert!(editor.undo());
        assert_eq!(
            editor.project().device(original).unwrap().state(),
            &crate::default_instrument_state().unwrap()
        );

        editor.execute(Command::RemoveInstrument { track }).unwrap();
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );
        assert!(editor.undo());
        assert_eq!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id(),
            original
        );
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        let replacement = match editor
            .execute(Command::InsertInstrument {
                track,
                state: custom.clone(),
            })
            .unwrap()
        {
            crate::Created::Device(id) => id,
            other => panic!("unexpected edit result: {other:?}"),
        };
        assert!(replacement > original);
        assert_eq!(
            editor.project().device(replacement).unwrap().state(),
            &custom
        );
        assert!(editor.undo());
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );

        let before = editor.project().clone();
        let mut snapshot = before.snapshot();
        snapshot.next_id = u64::MAX;
        let mut exhausted =
            crate::Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
        let before_exhausted = exhausted.project().clone();
        assert!(
            exhausted
                .execute(Command::InsertInstrument {
                    track,
                    state: custom,
                })
                .is_err()
        );
        assert_eq!(*exhausted.project(), before_exhausted);
        assert!(!exhausted.can_undo());
    }
}
