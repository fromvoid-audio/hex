//! Track name origin: automatic (hex-generated) versus user-set names.
//!
//! A track name is *automatic* when hex chose it: the model-owned default
//! `track N` from [`Command::InsertDefaultTrack`], or a device-derived name from
//! [`Command::InsertDefaultTrackWithDevice`],
//! [`Command::InsertDefaultTrackWithDeviceAndGain`], and [`Command::LoadInstrument`].
//! Every other creation path stores its caller-supplied name as user-set, and
//! [`Command::RenameTrack`] always makes a name user-set, even when the new text
//! equals the previous automatic one. Origin is an explicit stored flag; names
//! are never pattern-matched, so a user's deliberate `track 3` stays put.
//!
//! Loading a browser instrument (filling the empty slot or replacing the
//! current instrument) renames an automatic-named track to the instrument's
//! display name in the same history edit, so one Undo restores both. The name
//! stays automatic afterwards, so a later instrument load renames it again,
//! matching Live, which keeps following the device only while the name is
//! device-derived. Effects never rename, and removing an instrument keeps the
//! current name. Duplicated tracks copy the origin with the name.

use crate::{Command, Created, DeviceId, Invalid, Project, TrackDeviceSlot, TrackId};

/// Model-owned default name for the next created track.
pub(crate) fn default_name(project: &Project) -> String {
    format!("track {}", project.tracks.len().saturating_add(1))
}

pub(crate) fn insert_default_track(
    project: &mut Project,
    before_track: Option<TrackId>,
) -> Result<TrackId, Invalid> {
    let name = default_name(project);
    let track = crate::edit::insert_track_before(project, name, before_track)?;
    project.track_mut(track)?.automatic_name = true;
    Ok(track)
}

pub(crate) fn insert_default_track_with_device(
    project: &mut Project,
    before_track: Option<TrackId>,
    slot: TrackDeviceSlot,
    state: hex_lattice_contract::NativeDeviceState,
    device_name: String,
) -> Result<Created, Invalid> {
    crate::edit::name_valid(&device_name)?;
    let name = match slot {
        TrackDeviceSlot::Instrument => device_name,
        TrackDeviceSlot::Effect => default_name(project),
    };
    let created = project.apply(Command::InsertTrackWithDevice {
        name,
        before_track,
        slot,
        state,
    })?;
    if let Created::Track(track) = created {
        project.track_mut(track)?.automatic_name = true;
    }
    Ok(created)
}

pub(crate) fn load_instrument(
    project: &mut Project,
    track: TrackId,
    replacing: Option<DeviceId>,
    state: hex_lattice_contract::NativeDeviceState,
    device_name: String,
) -> Result<Created, Invalid> {
    crate::edit::name_valid(&device_name)?;
    let current = project
        .track(track)
        .ok_or(Invalid("track no longer exists"))?
        .instrument()
        .map(crate::Device::id);
    if current != replacing {
        return Err(Invalid("instrument replacement target changed"));
    }
    let created = match replacing {
        Some(device) => project.apply(Command::ReplaceDeviceState { device, state })?,
        None => project.apply(Command::InsertInstrument { track, state })?,
    };
    let target = project.track_mut(track)?;
    if target.automatic_name {
        target.name = device_name;
    }
    Ok(created)
}
