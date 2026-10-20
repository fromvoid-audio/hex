//! Borrowed UI queries on the actual retained snapshot. Private model fields
//! cannot be extracted, cloned into an unregistered owner, or kept past a read.
//! These queries confer no source, storage, or publication authority.
use super::SnapshotRead;
use hex_project::{ClipId, DeviceId, ReturnId, TrackId};

/// A model query tied to one existing family read, never an owning Project.
///
/// ```compile_fail
/// fn escape(read: &hex_session::snapshot_exports::SnapshotReadCapability) {
///     let _model = read.read_with(|snapshot| snapshot.model());
/// }
/// ```
/// ```compile_fail
/// fn extract(model: hex_session::snapshot_exports::SnapshotProjectRead<'_>) {
///     let _: hex_project::Project = model.clone();
/// }
/// ```
#[derive(Clone, Copy)]
pub struct SnapshotProjectRead<'a> {
    project: &'a hex_project::Project,
}
#[derive(Clone, Copy)]
pub struct SnapshotTrackRead<'a> {
    track: &'a hex_project::Track,
}
#[derive(Clone, Copy)]
pub struct SnapshotClipRead<'a> {
    clip: &'a hex_project::Clip,
}
#[derive(Clone, Copy)]
pub struct SnapshotDeviceRead<'a> {
    device: &'a hex_project::Device,
}
#[derive(Clone, Copy)]
pub struct SnapshotReturnRead<'a> {
    track: &'a hex_project::ReturnTrack,
}

impl<'a> SnapshotRead<'a> {
    pub fn model(self) -> SnapshotProjectRead<'a> {
        SnapshotProjectRead {
            project: &self.snapshot.project,
        }
    }
    pub fn timeline_frames(self) -> u64 {
        self.snapshot.timeline_frames
    }
    pub fn can_undo(self) -> bool {
        self.snapshot.can_undo
    }
    pub fn can_redo(self) -> bool {
        self.snapshot.can_redo
    }
    pub fn unsaved(self) -> bool {
        self.snapshot.unsaved
    }
    pub fn document_path(self) -> Option<&'a std::path::Path> {
        self.snapshot.document_path.as_deref()
    }
}
impl<'a> SnapshotProjectRead<'a> {
    pub fn sample_rate(self) -> u32 {
        self.project.sample_rate()
    }
    pub fn loop_region(self) -> Option<hex_project::LoopRegion> {
        self.project.loop_region()
    }
    pub fn master_gain(self) -> f32 {
        self.project.master_gain()
    }
    pub fn tracks(self) -> impl ExactSizeIterator<Item = SnapshotTrackRead<'a>> + 'a {
        self.project
            .tracks()
            .iter()
            .map(|track| SnapshotTrackRead { track })
    }
    pub fn track(self, id: TrackId) -> Option<SnapshotTrackRead<'a>> {
        self.project
            .track(id)
            .map(|track| SnapshotTrackRead { track })
    }
    pub fn returns(self) -> impl ExactSizeIterator<Item = SnapshotReturnRead<'a>> + 'a {
        self.project
            .return_tracks()
            .iter()
            .map(|track| SnapshotReturnRead { track })
    }
    pub fn device(self, id: DeviceId) -> Option<SnapshotDeviceRead<'a>> {
        self.project
            .device(id)
            .map(|device| SnapshotDeviceRead { device })
    }
}
impl<'a> SnapshotTrackRead<'a> {
    pub fn id(self) -> TrackId {
        self.track.id()
    }
    pub fn name(self) -> &'a str {
        self.track.name()
    }
    pub fn gain(self) -> f32 {
        self.track.gain()
    }
    pub fn balance(self) -> f32 {
        self.track.balance()
    }
    pub fn muted(self) -> bool {
        self.track.muted()
    }
    pub fn soloed(self) -> bool {
        self.track.soloed()
    }
    pub fn clips(self) -> impl ExactSizeIterator<Item = SnapshotClipRead<'a>> + 'a {
        self.track
            .clips()
            .iter()
            .map(|clip| SnapshotClipRead { clip })
    }
    pub fn clip(self, id: ClipId) -> Option<SnapshotClipRead<'a>> {
        self.track
            .clips()
            .iter()
            .find(|clip| clip.id() == id)
            .map(|clip| SnapshotClipRead { clip })
    }
    pub fn devices(self) -> impl ExactSizeIterator<Item = SnapshotDeviceRead<'a>> + 'a {
        self.track
            .devices()
            .iter()
            .map(|device| SnapshotDeviceRead { device })
    }
    pub fn instrument(self) -> Option<SnapshotDeviceRead<'a>> {
        self.track
            .instrument()
            .map(|device| SnapshotDeviceRead { device })
    }
    /// GainPoint is Copy scalar metadata; neither an owning lane nor its Vec is exposed.
    pub fn gain_points(self) -> &'a [hex_project::GainPoint] {
        self.track.gain_envelope()
    }
}
impl<'a> SnapshotClipRead<'a> {
    pub fn id(self) -> ClipId {
        self.clip.id()
    }
    pub fn name(self) -> &'a str {
        self.clip.name()
    }
    pub fn asset(self) -> hex_project::AssetId {
        self.clip.asset()
    }
    pub fn start(self) -> u64 {
        self.clip.start()
    }
    pub fn duration(self) -> u64 {
        self.clip.duration()
    }
    pub fn source_offset(self) -> u64 {
        self.clip.source_offset()
    }
    pub fn source_length(self) -> u64 {
        self.clip.source_length()
    }
    pub fn looping(self) -> bool {
        self.clip.looping()
    }
    pub fn gain(self) -> hex_project::ClipGain {
        self.clip.gain()
    }
    pub fn fade(self) -> hex_project::ClipFade {
        self.clip.fade()
    }
}
impl SnapshotDeviceRead<'_> {
    pub fn id(self) -> DeviceId {
        self.device.id()
    }
    pub fn type_id(self) -> hex_lattice_contract::DeviceTypeId {
        self.device.state().type_id()
    }
    pub fn bypassed(self) -> bool {
        self.device.bypassed()
    }
}
impl<'a> SnapshotReturnRead<'a> {
    pub fn id(self) -> ReturnId {
        self.track.id()
    }
    pub fn name(self) -> &'a str {
        self.track.name()
    }
    pub fn gain(self) -> f32 {
        self.track.gain()
    }
    pub fn muted(self) -> bool {
        self.track.muted()
    }
    pub fn devices(self) -> impl ExactSizeIterator<Item = SnapshotDeviceRead<'a>> + 'a {
        self.track
            .devices()
            .iter()
            .map(|device| SnapshotDeviceRead { device })
    }
}

#[cfg(test)]
mod tests;
