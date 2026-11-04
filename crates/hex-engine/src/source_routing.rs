//! Bounded immutable contributor/receiver routing for retained native sources.
//!
//! A binding contributes MIDI; it does not authorize a second generator or audio
//! path. Only the explicit receiver injects the combined source output into its
//! existing local chain. A control-allocated scalar table avoids large revision
//! copies on preparation stacks. Reads perform no callback map construction,
//! allocation or ownership transfer; the existing revision owns reclamation.
use crate::Invalid;
use hex_project::{MAX_TRACKS, Project, SourceId, TrackId};

/// Canonical source routing prepared alongside the ordinary track inventory.
/// Retained sources without an audio receiver produce no implicit Main path.
#[derive(Clone, Debug)]
struct SourceRoutingData {
    tracks: [Option<TrackId>; MAX_TRACKS],
    sources: [Option<SourceId>; MAX_TRACKS],
    receivers: [Option<u16>; MAX_TRACKS],
    contributors: [u16; MAX_TRACKS],
    ranges: [(u16, u16); MAX_TRACKS],
    shared: [bool; MAX_TRACKS],
    voice_envelopes: [u16; MAX_TRACKS],
    len: usize,
}
impl Default for SourceRoutingData {
    fn default() -> Self {
        Self {
            tracks: [None; MAX_TRACKS],
            sources: [None; MAX_TRACKS],
            receivers: [None; MAX_TRACKS],
            contributors: [0; MAX_TRACKS],
            ranges: [(0, 0); MAX_TRACKS],
            shared: [false; MAX_TRACKS],
            voice_envelopes: [0; MAX_TRACKS],
            len: 0,
        }
    }
}
impl SourceRoutingData {
    /// Build bounded receiver ranges after contributor bindings are resolved.
    /// A remote receiver requires shared scheduling even for a single contributor.
    fn finish(&mut self) -> Result<(), Invalid> {
        self.shared = [false; MAX_TRACKS];
        for index in 0..self.len {
            if let Some(source) = self.sources[index] {
                let mut count = 0;
                for other in 0..self.len {
                    if self.sources[other] == Some(source) {
                        if self.receivers[other] != self.receivers[index] {
                            return Err(Invalid("source contributors disagree on receiver"));
                        }
                        count += 1;
                    }
                }
                self.shared[index] = count > 1;
            }
        }
        let mut next = 0usize;
        for receiver in 0..self.len {
            let begin =
                u16::try_from(next).map_err(|_| Invalid("source contributor index overflow"))?;
            let mut source = None;
            for contributor in 0..self.len {
                if self.receivers[contributor].map(usize::from) == Some(receiver) {
                    let bound = self.sources[contributor]
                        .ok_or(Invalid("source contributor binding is missing"))?;
                    if source.is_some_and(|previous| previous != bound) {
                        return Err(Invalid("source receiver has conflicting sources"));
                    }
                    source = Some(bound);
                    let slot = self
                        .contributors
                        .get_mut(next)
                        .ok_or(Invalid("source contributor limit reached"))?;
                    *slot = u16::try_from(contributor)
                        .map_err(|_| Invalid("source contributor index overflow"))?;
                    next += 1;
                }
            }
            let end =
                u16::try_from(next).map_err(|_| Invalid("source contributor index overflow"))?;
            self.ranges[receiver] = (begin, end);
            if source.is_some() && self.sources[receiver].is_some_and(|bound| Some(bound) != source)
            {
                return Err(Invalid("source receiver has a different binding"));
            }
            if end - begin > 1 || (end > begin && self.sources[receiver] != source) {
                self.shared[receiver] = true;
                for &contributor in &self.contributors[usize::from(begin)..usize::from(end)] {
                    self.shared[usize::from(contributor)] = true;
                }
            }
        }
        if self.receivers[..self.len]
            .iter()
            .flatten()
            .any(|receiver| usize::from(*receiver) >= self.len)
        {
            return Err(Invalid("source receiver index is out of range"));
        }
        Ok(())
    }
}
/// Control-allocated bounded source routing. Empty audio-only graphs own no
/// child allocation; source graphs keep their scalar table off preparation
/// stacks. The revision retains it until control-side graph reclamation.
#[derive(Clone, Debug, Default)]
pub struct PreparedSourceRouting {
    data: Option<Box<SourceRoutingData>>,
}

impl PreparedSourceRouting {
    pub(crate) fn prepare(project: &Project) -> Result<Self, Invalid> {
        if project.tracks().len() > MAX_TRACKS {
            return Err(Invalid("source contributor limit reached"));
        }
        if !project
            .tracks()
            .iter()
            .any(|track| track.source_id().is_some())
        {
            return Ok(Self::default());
        }
        let mut plan = SourceRoutingData {
            len: project.tracks().len(),
            ..SourceRoutingData::default()
        };
        for (index, track) in project.tracks().iter().enumerate() {
            plan.tracks[index] = Some(track.id());
            plan.sources[index] = track.source_id();
            if let Some(source_id) = track.source_id() {
                let source = project
                    .native_source(source_id)
                    .ok_or(Invalid("source binding is missing"))?;
                if let Some(receiver) = source.receiver() {
                    let receiver_index = project
                        .tracks()
                        .iter()
                        .position(|t| t.id() == receiver && t.source_id() == Some(source_id))
                        .ok_or(Invalid("source receiver is not a contributor"))?;
                    plan.receivers[index] = Some(
                        u16::try_from(receiver_index)
                            .map_err(|_| Invalid("source receiver index overflow"))?,
                    );
                }
            }
        }
        plan.finish()?;
        Ok(Self {
            data: Some(Box::new(plan)),
        })
    }
    pub(crate) fn prepare_for_playback(
        canonical: &Project,
        arrangement: &crate::Project,
        session: &crate::PreparedSessionInventory,
    ) -> Result<Self, Invalid> {
        let mut routing = Self::prepare(canonical)?;
        for (index, track) in arrangement.tracks().iter().enumerate() {
            if routing.source(index).is_some() {
                let id = track
                    .canonical_track_id()
                    .ok_or(Invalid("source track id is missing"))?;
                routing
                    .data
                    .as_mut()
                    .ok_or(Invalid("source routing table is missing"))?
                    .voice_envelopes[index] =
                    u16::try_from(session.contributor_voice_envelope(id, track)?)
                        .map_err(|_| Invalid("source voice envelope overflow"))?;
            }
        }
        Ok(routing)
    }

    pub(crate) fn contributor_voice_envelope(&self, contributor: usize) -> usize {
        self.data
            .as_ref()
            .and_then(|data| data.voice_envelopes.get(contributor))
            .copied()
            .map_or(0, usize::from)
    }
    pub(crate) fn construction_bytes(project: &Project) -> usize {
        if project
            .tracks()
            .iter()
            .any(|track| track.source_id().is_some())
        {
            std::mem::size_of::<SourceRoutingData>()
        } else {
            0
        }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.data
            .as_ref()
            .map_or(0, |_| std::mem::size_of::<SourceRoutingData>())
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        if let Some(data) = &self.data {
            ledger.bytes(
                crate::audio_storage::Owner::exclusive(data.as_ref()),
                crate::audio_storage::Location("source-routing", 0),
                std::mem::size_of::<SourceRoutingData>(),
            )?;
        }
        Ok(())
    }
    pub fn source(&self, track_index: usize) -> Option<SourceId> {
        self.data
            .as_ref()?
            .sources
            .get(track_index)
            .copied()
            .flatten()
    }
    pub fn receiver_index(&self, contributor_index: usize) -> Option<usize> {
        self.data
            .as_ref()?
            .receivers
            .get(contributor_index)
            .copied()
            .flatten()
            .map(usize::from)
    }
    pub fn receiver(&self, contributor: TrackId) -> Option<TrackId> {
        let data = self.data.as_ref()?;
        let index = data.tracks[..data.len]
            .iter()
            .position(|track| *track == Some(contributor))?;
        self.receiver_index(index)
            .and_then(|receiver| data.tracks.get(receiver).copied().flatten())
    }
    pub fn contributors(&self, receiver_index: usize) -> &[u16] {
        self.data
            .as_ref()
            .and_then(|data| {
                data.ranges.get(receiver_index).and_then(|&(start, end)| {
                    data.contributors.get(usize::from(start)..usize::from(end))
                })
            })
            .unwrap_or(&[])
    }
    /// Whether this track participates in a route requiring shared scheduling,
    /// including a receiver that does not itself contribute MIDI.
    pub fn is_shared(&self, track_index: usize) -> bool {
        self.data
            .as_ref()
            .and_then(|data| data.shared.get(track_index))
            .copied()
            .unwrap_or(false)
    }
    pub fn has_shared_sources(&self) -> bool {
        self.data
            .as_ref()
            .is_some_and(|data| data.shared[..data.len].iter().any(|shared| *shared))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};

    fn shared_plan() -> SourceRoutingData {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::AddMidiTrack {
                name: "receiver".into(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        let id =
            hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
        editor
            .execute(Command::InsertInstrument {
                track: receiver,
                state: hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap(),
            })
            .unwrap();
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        for name in ["first", "second"] {
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack { name: name.into() })
                .unwrap()
            else {
                panic!("contributor")
            };
            editor
                .execute(Command::BindTrackSource {
                    track,
                    source: Some(source),
                })
                .unwrap();
        }
        *PreparedSourceRouting::prepare(editor.project())
            .unwrap()
            .data
            .unwrap()
    }

    #[test]
    fn receiver_and_contributor_roles_are_independent() {
        let mut data = shared_plan();
        data.sources[0] = None;
        data.receivers[0] = None;
        data.sources[2] = None;
        data.receivers[2] = None;
        data.finish().unwrap();
        let routing = PreparedSourceRouting {
            data: Some(Box::new(data)),
        };
        assert_eq!(routing.contributors(0), &[1]);
        assert_eq!(routing.receiver_index(1), Some(0));
        assert!(routing.is_shared(0));
        assert!(routing.is_shared(1));
        assert!(!routing.is_shared(2));
        assert!(routing.contributors(MAX_TRACKS).is_empty());
        assert_eq!(routing.receiver_index(MAX_TRACKS), None);
    }

    #[test]
    fn contributor_ranges_cover_bound_remote_and_unassigned_receivers() {
        let mut data = shared_plan();
        data.finish().unwrap();
        assert_eq!(&data.contributors[..3], &[0, 1, 2]);
        data.sources[0] = None;
        data.receivers[0] = None;
        data.finish().unwrap();
        assert_eq!(&data.contributors[..2], &[1, 2]);
        data.receivers[1] = None;
        assert!(data.finish().is_err());
        data.receivers[2] = None;
        data.finish().unwrap();
        assert_eq!(data.ranges[0], (0, 0));
        assert!(data.shared[1] && data.shared[2]);
        data.receivers[1] = Some(data.len as u16);
        assert!(data.finish().is_err());
        data.receivers[1] = None;
        data.sources[1] = None;
        data.sources[2] = None;
        data.finish().unwrap();
        assert!(data.ranges[..data.len].iter().all(|range| *range == (0, 0)));
        assert!(!data.shared[..data.len].iter().any(|shared| *shared));
    }
}
