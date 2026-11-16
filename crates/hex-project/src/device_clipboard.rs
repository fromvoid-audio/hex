//! Bounded control-side device clipboard. Captures authored device/source state;
//! playback instances and UI selection remain owned by their respective hosts.
use crate::{
    Asset, AssetId, Command, Created, Device, DeviceId, Invalid, Project, ReturnId, TrackDeviceRef,
    TrackId,
};
use std::{collections::BTreeSet, sync::Arc};

/// A single canonical device chain, independent of UI focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceChainOwner {
    Track(TrackId),
    Main,
    Return(ReturnId),
}

/// Bounded, duplicate-free membership. Capture restores canonical chain order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceSelectionIds(Arc<[DeviceId]>);
impl DeviceSelectionIds {
    pub fn new(ids: &[DeviceId]) -> Result<Self, Invalid> {
        if ids.is_empty() || ids.len() > crate::MAX_DEVICES_PER_TRACK + 1 {
            return Err(Invalid("device selection must be nonempty and bounded"));
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(ids.len())
            .map_err(|_| Invalid("could not reserve device selection"))?;
        for id in ids {
            if values.contains(id) {
                return Err(Invalid("device selection contains duplicates"));
            }
            values.push(*id);
        }
        Ok(Self(values.into()))
    }
    pub fn ids(&self) -> &[DeviceId] {
        &self.0
    }
}

#[derive(Clone, Debug)]
enum Member {
    Effect(Box<Device>),
    Instrument(crate::native_sources::SourceBinding),
}
impl Member {
    fn device(&self) -> &Device {
        match self {
            Self::Effect(device) => device,
            Self::Instrument(binding) => binding,
        }
    }

    fn authored_devices(&self) -> impl Iterator<Item = &Device> {
        let (effect, source) = match self {
            Self::Effect(device) => (Some(device.as_ref()), None),
            Self::Instrument(binding) => (None, Some(binding)),
        };
        effect
            .into_iter()
            .chain(source.into_iter().flat_map(|binding| binding.devices()))
    }
}

/// Complete authored state, restricted to the project and media it was captured from.
#[derive(Clone, Debug)]
pub struct DeviceClipboard {
    sample_rate: u32,
    members: Vec<Member>,
    assets: Vec<(AssetId, Asset)>,
}

impl Project {
    /// Capture selected units and descendants once in canonical signal order.
    /// Paste assigns fresh IDs and reroots a subtree copied without its parent.
    pub fn capture_devices(
        &self,
        owner: DeviceChainOwner,
        selected: &DeviceSelectionIds,
    ) -> Result<DeviceClipboard, Invalid> {
        let mut members = Vec::new();
        members
            .try_reserve_exact(crate::MAX_DEVICES_PER_TRACK + 1)
            .map_err(|_| Invalid("could not reserve device clipboard"))?;
        match owner {
            DeviceChainOwner::Track(id) => {
                let track = self
                    .track(id)
                    .ok_or(Invalid("device track no longer exists"))?;
                for node in track.ordered_device_chain() {
                    match node {
                        TrackDeviceRef::Instrument(device)
                            if selected.ids().contains(&device.id()) =>
                        {
                            let binding = track
                                .instrument
                                .as_ref()
                                .ok_or(Invalid("instrument no longer exists"))?;
                            members.push(Member::Instrument(binding.clone()));
                        }
                        TrackDeviceRef::Effect(device) if included(device, selected, &members) => {
                            members.push(Member::Effect(Box::new(device.clone())));
                        }
                        _ => {}
                    }
                }
            }
            DeviceChainOwner::Main => capture_effects(self.main_devices(), selected, &mut members),
            DeviceChainOwner::Return(id) => {
                let track = self
                    .return_tracks()
                    .iter()
                    .find(|track| track.id() == id)
                    .ok_or(Invalid("return no longer exists"))?;
                capture_effects(track.devices(), selected, &mut members);
            }
        }
        if selected
            .ids()
            .iter()
            .any(|id| !members.iter().any(|member| member.device().id() == *id))
        {
            return Err(Invalid("selected device no longer belongs to the chain"));
        }
        let mut asset_ids = BTreeSet::new();
        for device in members.iter().flat_map(Member::authored_devices) {
            asset_ids.extend(
                device
                    .authored_states()
                    .flat_map(|state| state.asset_references())
                    .map(|item| AssetId(item.asset.value())),
            );
        }
        let assets = asset_ids
            .into_iter()
            .map(|id| {
                self.asset(id)
                    .cloned()
                    .map(|asset| (id, asset))
                    .ok_or(Invalid("copied device references missing media"))
            })
            .collect::<Result<_, _>>()?;
        Ok(DeviceClipboard {
            sample_rate: self.sample_rate(),
            members,
            assets,
        })
    }
}

// Validated preorder guarantees that an included parent precedes every child.
fn included(device: &Device, selected: &DeviceSelectionIds, members: &[Member]) -> bool {
    selected.ids().contains(&device.id())
        || device
            .parent()
            .is_some_and(|parent| members.iter().any(|member| member.device().id() == parent))
}
fn capture_effects(devices: &[Device], selected: &DeviceSelectionIds, members: &mut Vec<Member>) {
    for device in devices {
        if included(device, selected, members) {
            members.push(Member::Effect(Box::new(device.clone())));
        }
    }
}

pub(crate) fn remove(
    project: &mut Project,
    owner: DeviceChainOwner,
    selected: &DeviceSelectionIds,
) -> Result<Created, Invalid> {
    let captured = project.capture_devices(owner, selected)?;
    // An ancestor's RemoveDevice already removes its captured descendants.
    for member in &captured.members {
        if member.device().parent().is_some_and(|parent| {
            captured
                .members
                .iter()
                .any(|ancestor| ancestor.device().id() == parent)
        }) {
            continue;
        }
        let command = match (owner, member) {
            (DeviceChainOwner::Track(track), Member::Instrument(_)) => {
                Command::RemoveInstrument { track }
            }
            (_, Member::Effect(device)) => Command::RemoveDevice(device.id()),
            _ => return Err(Invalid("instrument destination is invalid")),
        };
        project.apply(command)?;
    }
    Ok(Created::None)
}

pub(crate) fn paste(
    project: &mut Project,
    clipboard: &DeviceClipboard,
    owner: DeviceChainOwner,
    before: Option<DeviceId>,
) -> Result<Created, Invalid> {
    if clipboard.sample_rate != project.sample_rate() {
        return Err(Invalid("device clipboard belongs to another sample rate"));
    }
    for (id, asset) in &clipboard.assets {
        if project.asset(*id) != Some(asset) {
            return Err(Invalid("copied media no longer matches the project"));
        }
    }
    preflight_paste(project, clipboard, owner)?;
    let mut remapped = Vec::new();
    remapped
        .try_reserve_exact(clipboard.members.len())
        .map_err(|_| Invalid("could not reserve device identity mapping"))?;
    let mut first = None;
    for member in &clipboard.members {
        // External parents are intentionally absent: a copied subunit is a root.
        let parent = member.device().parent().and_then(|parent| {
            remapped
                .iter()
                .find_map(|(old, new)| (*old == parent).then_some(*new))
        });
        let id = match (owner, member) {
            (DeviceChainOwner::Track(track), Member::Instrument(binding)) => {
                let target = project
                    .track(track)
                    .ok_or(Invalid("device track no longer exists"))?;
                if target.instrument.is_some() || target.is_group() {
                    return Err(Invalid("destination cannot accept an instrument"));
                }
                if crate::returns::device_count(project)? >= crate::MAX_DEVICES {
                    return Err(Invalid("project device limit reached"));
                }
                let copied = project.copy_source_binding(binding, track)?;
                let id = copied.id();
                let target = project.track_mut(track)?;
                target.instrument = Some(copied);
                target.instrument_position = Some(0);
                project.apply(Command::MoveTrackDevice {
                    device: id,
                    track,
                    before,
                })?;
                id
            }
            (DeviceChainOwner::Track(track), Member::Effect(device)) => {
                let Created::Device(id) = project.apply(Command::InsertDevice {
                    track,
                    before,
                    state: device.state().clone(),
                })?
                else {
                    return Err(Invalid("device insertion failed"));
                };
                copy_authored(project, id, device, parent)?;
                id
            }
            (DeviceChainOwner::Main, Member::Effect(device)) => {
                let Created::Device(id) = project.apply(Command::InsertMainDevice {
                    before,
                    state: device.state().clone(),
                })?
                else {
                    return Err(Invalid("device insertion failed"));
                };
                copy_authored(project, id, device, parent)?;
                id
            }
            (DeviceChainOwner::Return(return_track), Member::Effect(device)) => {
                let Created::Device(id) = project.apply(Command::InsertReturnDevice {
                    return_track,
                    before,
                    state: device.state().clone(),
                })?
                else {
                    return Err(Invalid("device insertion failed"));
                };
                copy_authored(project, id, device, parent)?;
                id
            }
            _ => return Err(Invalid("destination cannot accept an instrument")),
        };
        remapped.push((member.device().id(), id));
        first.get_or_insert(id);
    }
    first
        .map(Created::Device)
        .ok_or(Invalid("device clipboard is empty"))
}
/// Quote all expanded nodes before the first insertion reserves any new IDs.
fn preflight_paste(
    project: &Project,
    clipboard: &DeviceClipboard,
    owner: DeviceChainOwner,
) -> Result<(), Invalid> {
    let effects = clipboard
        .members
        .iter()
        .filter(|member| matches!(member, Member::Effect(_)))
        .count();
    let instruments = clipboard
        .members
        .len()
        .checked_sub(effects)
        .ok_or(Invalid("device count underflow"))?;
    let current = match owner {
        DeviceChainOwner::Track(id) => {
            let track = project
                .track(id)
                .ok_or(Invalid("device track no longer exists"))?;
            if instruments > 1
                || (instruments > 0 && (track.instrument.is_some() || track.is_group()))
            {
                return Err(Invalid("destination cannot accept an instrument"));
            }
            track.devices().len()
        }
        DeviceChainOwner::Main => {
            if instruments > 0 {
                return Err(Invalid("destination cannot accept an instrument"));
            }
            project.main_devices().len()
        }
        DeviceChainOwner::Return(id) => {
            if instruments > 0 {
                return Err(Invalid("destination cannot accept an instrument"));
            }
            project
                .return_tracks()
                .iter()
                .find(|track| track.id() == id)
                .ok_or(Invalid("return no longer exists"))?
                .devices()
                .len()
        }
    };
    if current
        .checked_add(effects)
        .is_none_or(|count| count > crate::MAX_DEVICES_PER_TRACK)
    {
        return Err(Invalid("track device limit reached"));
    }
    let copied_devices = clipboard
        .members
        .iter()
        .flat_map(Member::authored_devices)
        .count();
    if crate::returns::device_count(project)?
        .checked_add(copied_devices)
        .is_none_or(|count| count > crate::MAX_DEVICES)
    {
        return Err(Invalid("project device limit reached"));
    }
    Ok(())
}

fn copy_authored(
    project: &mut Project,
    id: DeviceId,
    source: &Device,
    parent: Option<DeviceId>,
) -> Result<(), Invalid> {
    project.edit_device(id, |target| {
        let mut copied = source.clone();
        copied.id = id;
        copied.parent = parent;
        *target = copied;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Editor, Project};
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};

    #[test]
    fn effects_copy_across_main_and_return_chains() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 12).unwrap();
        let state = NativeDeviceState::new(DeviceTypeId::from_static("fixture.effect"), 1, vec![1])
            .unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("main device");
        };
        let Created::ReturnTrack(return_track) = editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return");
        };
        let clipboard = Arc::new(
            editor
                .project()
                .capture_devices(
                    DeviceChainOwner::Main,
                    &DeviceSelectionIds::new(&[device]).unwrap(),
                )
                .unwrap(),
        );
        editor
            .execute(Command::PasteDevices {
                clipboard: Arc::clone(&clipboard),
                owner: DeviceChainOwner::Return(return_track),
                before: None,
            })
            .unwrap();
        let copied = editor
            .project()
            .return_tracks()
            .iter()
            .find(|track| track.id() == return_track)
            .unwrap()
            .devices()[0]
            .clone();
        assert_ne!(device, copied.id());
        assert_eq!(editor.project().main_devices()[0].state(), copied.state());
        let return_clipboard = Arc::new(
            editor
                .project()
                .capture_devices(
                    DeviceChainOwner::Return(return_track),
                    &DeviceSelectionIds::new(&[copied.id()]).unwrap(),
                )
                .unwrap(),
        );
        editor
            .execute(Command::PasteDevices {
                clipboard: return_clipboard,
                owner: DeviceChainOwner::Main,
                before: Some(device),
            })
            .unwrap();
        assert_eq!(editor.project().main_devices().len(), 2);
        assert_eq!(editor.project().main_devices()[1].id(), device);
    }

    #[test]
    fn instrument_copy_is_independent_and_rejects_incompatible_destinations_atomically() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 20).unwrap();
        let Created::Track(source) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("source");
        };
        let Created::Track(destination) = editor
            .execute(Command::AddTrack {
                name: "destination".into(),
            })
            .unwrap()
        else {
            panic!("destination");
        };
        let Created::Device(instrument) = editor
            .execute(Command::InsertInstrument {
                track: source,
                state: crate::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("instrument");
        };
        let clipboard = Arc::new(
            editor
                .project()
                .capture_devices(
                    DeviceChainOwner::Track(source),
                    &DeviceSelectionIds::new(&[instrument]).unwrap(),
                )
                .unwrap(),
        );
        let initial = editor.project().clone();
        assert!(
            editor
                .execute(Command::PasteDevices {
                    clipboard: Arc::clone(&clipboard),
                    owner: DeviceChainOwner::Main,
                    before: None
                })
                .is_err()
        );
        assert_eq!(editor.project(), &initial);
        editor
            .execute(Command::PasteDevices {
                clipboard: Arc::clone(&clipboard),
                owner: DeviceChainOwner::Track(destination),
                before: None,
            })
            .unwrap();
        let src = editor.project().track(source).unwrap();
        let dst = editor.project().track(destination).unwrap();
        assert_ne!(src.source_id(), dst.source_id());
        assert_ne!(
            src.instrument().unwrap().id(),
            dst.instrument().unwrap().id()
        );
        assert_eq!(
            src.instrument().unwrap().state(),
            dst.instrument().unwrap().state()
        );
        let before = editor.project().clone();
        assert!(
            editor
                .execute(Command::PasteDevices {
                    clipboard,
                    owner: DeviceChainOwner::Track(destination),
                    before: None
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(editor.undo());
        assert!(
            editor
                .project()
                .track(destination)
                .unwrap()
                .instrument()
                .is_none()
        );
    }

    #[test]
    fn group_clipboard_preserves_order_state_and_history_atomically() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 20).unwrap();
        let Created::Track(source) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let Created::Track(destination) = editor
            .execute(Command::AddTrack {
                name: "destination".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let state = |name| {
            NativeDeviceState::new(DeviceTypeId::from_static(name), 1, vec![1, 2, 3]).unwrap()
        };
        let mut ids = Vec::new();
        for name in ["fixture.one", "fixture.two"] {
            let Created::Device(id) = editor
                .execute(Command::InsertDevice {
                    track: source,
                    before: None,
                    state: state(name),
                })
                .unwrap()
            else {
                panic!("device");
            };
            ids.push(id);
        }
        editor
            .execute(Command::SetDeviceBypass {
                device: ids[1],
                bypassed: true,
            })
            .unwrap();
        let comparison =
            crate::DeviceComparison::new(crate::DeviceVariant::B, state("fixture.one"));
        editor
            .execute(Command::ReplaceDeviceComparison {
                device: ids[0],
                state: state("fixture.one"),
                comparison: Some(comparison),
            })
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device: ids[0],
                parameter: hex_lattice_contract::ParameterId::from_static(1),
                points: vec![crate::AutomationPoint {
                    beat: crate::BeatTime::ZERO,
                    value: crate::AutomationValue::new(0.25).unwrap(),
                    outgoing: crate::AutomationInterpolation::Linear,
                }],
            })
            .unwrap();
        let selection = DeviceSelectionIds::new(&[ids[1], ids[0]]).unwrap();
        let clipboard = Arc::new(
            editor
                .project()
                .capture_devices(DeviceChainOwner::Track(source), &selection)
                .unwrap(),
        );
        editor
            .execute(Command::PasteDevices {
                clipboard: Arc::clone(&clipboard),
                owner: DeviceChainOwner::Track(destination),
                before: None,
            })
            .unwrap();
        let result = editor.project().track(destination).unwrap().devices();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].state(), &state("fixture.one"));
        assert_eq!(result[1].state(), &state("fixture.two"));
        assert_eq!(
            result[0].comparison(),
            editor.project().device(ids[0]).unwrap().comparison()
        );
        assert_eq!(
            result[0].automation(),
            editor.project().device(ids[0]).unwrap().automation()
        );
        assert!(!result[0].bypassed());
        assert!(result[1].bypassed());
        assert!(result.iter().all(|device| !ids.contains(&device.id())));
        assert!(editor.undo());
        assert!(
            editor
                .project()
                .track(destination)
                .unwrap()
                .devices()
                .is_empty()
        );
        assert!(editor.redo());
        assert_eq!(
            editor.project().track(destination).unwrap().devices().len(),
            2
        );
        let before = editor.project().clone();
        let result = editor.execute(Command::PasteDevices {
            clipboard,
            owner: DeviceChainOwner::Track(destination),
            before: Some(ids[0]),
        });
        assert!(result.is_err());
        assert_eq!(editor.project(), &before);
        editor
            .execute(Command::RemoveDevices {
                owner: DeviceChainOwner::Track(source),
                selected: selection,
            })
            .unwrap();
        assert!(editor.project().track(source).unwrap().devices().is_empty());
        assert!(editor.undo());
        assert_eq!(editor.project().track(source).unwrap().devices().len(), 2);
    }
}

#[cfg(test)]
#[path = "device_clipboard_unit_tests.rs"]
mod unit_tests;
