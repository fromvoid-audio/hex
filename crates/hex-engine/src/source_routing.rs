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
                let count = project.source_contributors(source_id).count();
                plan.shared[index] = count > 1;
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
        let mut next = 0usize;
        for receiver in 0..plan.len {
            let begin =
                u16::try_from(next).map_err(|_| Invalid("source contributor index overflow"))?;
            for contributor in 0..plan.len {
                if plan.receivers[contributor].map(usize::from) == Some(receiver) {
                    let slot = plan
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
            plan.ranges[receiver] = (begin, end);
        }
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
    pub fn is_shared(&self, contributor_index: usize) -> bool {
        self.data
            .as_ref()
            .and_then(|data| data.shared.get(contributor_index))
            .copied()
            .unwrap_or(false)
    }
    pub fn has_shared_sources(&self) -> bool {
        self.data
            .as_ref()
            .is_some_and(|data| data.shared[..data.len].iter().any(|shared| *shared))
    }
}
