//! Typed, bounded canonical audio inputs and authored MIDI source selection.
use crate::{Created, Invalid, Project, RouteId, RoutingOwner, Snapshot, TrackId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_AUDIO_FEEDS: usize = crate::MAX_AUTHORED_ROUTES;
pub const MAX_MIDI_ROUTES: usize = crate::MAX_TRACKS;

/// Additive post-fader stereo signal into another track before its effect prefix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioFeed {
    id: RouteId,
    source: RoutingOwner,
    destination: TrackId,
    gain: f32,
}
impl AudioFeed {
    pub fn id(self) -> RouteId {
        self.id
    }
    pub fn source(self) -> RoutingOwner {
        self.source
    }
    pub fn destination(self) -> TrackId {
        self.destination
    }
    pub fn gain(self) -> f32 {
        self.gain
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioFeedSnapshot {
    pub id: u64,
    pub source: RoutingOwner,
    pub destination: u64,
    pub gain: f32,
}

/// Selected incoming MIDI bus; the destination retains its own instrument/mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiRoute {
    id: RouteId,
    source: TrackId,
    destination: TrackId,
}
impl MidiRoute {
    pub fn id(self) -> RouteId {
        self.id
    }
    pub fn source(self) -> TrackId {
        self.source
    }
    pub fn destination(self) -> TrackId {
        self.destination
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiRouteSnapshot {
    pub id: u64,
    pub source: u64,
    pub destination: u64,
}

impl Project {
    pub fn audio_feeds(&self) -> &[AudioFeed] {
        &self.audio_feeds
    }
    pub fn midi_routes(&self) -> &[MidiRoute] {
        &self.midi_routes
    }
    /// Resolve a selected MIDI bus to its authored source. Validation guarantees
    /// an acyclic graph; the finite bound also protects callers during admission.
    pub fn midi_source_track(&self, destination: TrackId) -> TrackId {
        let mut source = destination;
        for _ in 0..crate::MAX_TRACKS {
            let Some(route) = self
                .midi_routes
                .iter()
                .find(|route| route.destination == source)
            else {
                break;
            };
            source = route.source;
        }
        source
    }
    pub(crate) fn set_audio_feed(
        &mut self,
        source: RoutingOwner,
        destination: TrackId,
        gain: f32,
    ) -> Result<Created, Invalid> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(Invalid("audio feed gain must be finite and within 0..2"));
        }
        self.validate_audio_owner(source)?;
        self.track(destination)
            .ok_or(Invalid("track no longer exists"))?;
        if let Some(index) = self
            .audio_feeds
            .iter()
            .position(|route| route.source == source && route.destination == destination)
        {
            if gain == 0.0 {
                self.audio_feeds.remove(index);
            } else {
                self.audio_feeds[index].gain = gain;
            }
        } else if gain != 0.0 {
            if self.audio_feeds.len() >= MAX_AUDIO_FEEDS {
                return Err(Invalid("audio feed limit reached"));
            }
            let id = RouteId(self.next_id);
            self.next_id = self
                .next_id
                .checked_add(1)
                .ok_or(Invalid("project id space exhausted"))?;
            self.audio_feeds.push(AudioFeed {
                id,
                source,
                destination,
                gain,
            });
        }
        self.validate_playable_routes()?;
        Ok(Created::None)
    }
    pub(crate) fn set_midi_source(
        &mut self,
        destination: TrackId,
        source: Option<TrackId>,
    ) -> Result<Created, Invalid> {
        self.track(destination)
            .ok_or(Invalid("track no longer exists"))?;
        if let Some(source) = source {
            self.track(source)
                .ok_or(Invalid("track no longer exists"))?;
        }
        if let Some(index) = self
            .midi_routes
            .iter()
            .position(|route| route.destination == destination)
        {
            if let Some(source) = source {
                self.midi_routes[index].source = source;
            } else {
                self.midi_routes.remove(index);
            }
        } else if let Some(source) = source {
            if self.midi_routes.len() >= MAX_MIDI_ROUTES {
                return Err(Invalid("midi route limit reached"));
            }
            let id = RouteId(self.next_id);
            self.next_id = self
                .next_id
                .checked_add(1)
                .ok_or(Invalid("project id space exhausted"))?;
            self.midi_routes.push(MidiRoute {
                id,
                source,
                destination,
            });
        }
        self.validate_playable_routes()?;
        Ok(Created::None)
    }
    fn validate_audio_owner(&self, owner: RoutingOwner) -> Result<(), Invalid> {
        match owner {
            RoutingOwner::Track(id) if self.tracks.iter().any(|track| track.id() == id) => Ok(()),
            RoutingOwner::Return(id) if self.returns.iter().any(|bus| bus.id() == id) => Ok(()),
            RoutingOwner::Main => Err(Invalid("main cannot feed a track input")),
            _ => Err(Invalid("routing owner no longer exists")),
        }
    }
    pub(crate) fn adopt_playable_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), Invalid> {
        if snapshot.audio_feeds.len() > MAX_AUDIO_FEEDS {
            return Err(Invalid("audio feed limit reached"));
        }
        if snapshot.midi_routes.len() > MAX_MIDI_ROUTES {
            return Err(Invalid("midi route limit reached"));
        }
        let mut ids = crate::return_feeds::existing_ids(snapshot);
        ids.extend(snapshot.return_feeds.iter().map(|feed| feed.id));
        let mut claim = |id| {
            if id == 0 || id >= snapshot.next_id || !ids.insert(id) {
                Err(Invalid(
                    "route id must be globally unique below allocator high-water",
                ))
            } else {
                Ok(())
            }
        };
        let mut audio = Vec::with_capacity(snapshot.audio_feeds.len());
        for feed in &snapshot.audio_feeds {
            claim(feed.id)?;
            audio.push(AudioFeed {
                id: RouteId(feed.id),
                source: feed.source,
                destination: TrackId(feed.destination),
                gain: feed.gain,
            });
        }
        let mut midi = Vec::with_capacity(snapshot.midi_routes.len());
        for route in &snapshot.midi_routes {
            claim(route.id)?;
            midi.push(MidiRoute {
                id: RouteId(route.id),
                source: TrackId(route.source),
                destination: TrackId(route.destination),
            });
        }
        self.audio_feeds = audio;
        self.midi_routes = midi;
        self.validate_playable_routes()
    }
    pub(crate) fn validate_playable_routes(&self) -> Result<(), Invalid> {
        if self.audio_feeds.is_empty() && self.midi_routes.is_empty() && self.sidechains.is_empty()
        {
            return Ok(());
        }
        if self.audio_feeds.len() > MAX_AUDIO_FEEDS || self.midi_routes.len() > MAX_MIDI_ROUTES {
            return Err(Invalid("authored routing edge limit reached"));
        }
        let nodes: BTreeSet<_> = self
            .tracks
            .iter()
            .map(|t| RoutingOwner::Track(t.id()))
            .chain(self.returns.iter().map(|b| RoutingOwner::Return(b.id())))
            .chain([RoutingOwner::Main])
            .collect();
        let audio = self.playable_audio_edges(None)?;
        dag(&nodes, &audio)?;
        let mut destinations = BTreeSet::new();
        let mut midi = BTreeSet::new();
        for route in &self.midi_routes {
            let source = self
                .track(route.source)
                .ok_or(Invalid("track no longer exists"))?;
            if source.is_group() {
                return Err(Invalid("group tracks do not carry midi"));
            }
            if self
                .track(route.destination)
                .ok_or(Invalid("track no longer exists"))?
                .instrument()
                .is_none()
            {
                return Err(Invalid("midi destination requires an instrument"));
            }
            if !destinations.insert(route.destination) {
                return Err(Invalid("duplicate midi destination"));
            }
            midi.insert((
                RoutingOwner::Track(route.source),
                RoutingOwner::Track(route.destination),
            ));
        }
        dag(&nodes, &midi)
    }

    pub(crate) fn playable_audio_edges(
        &self,
        skip_sidechain: Option<crate::DeviceId>,
    ) -> Result<BTreeSet<(RoutingOwner, RoutingOwner)>, Invalid> {
        let mut audio = BTreeSet::new();
        // Default group outputs participate in the same graph as authored
        // additive feeds/sends. A new hierarchy cannot hide a feedback cycle.
        for track in &self.tracks {
            if let Some(parent) = track.parent_group() {
                audio.insert((RoutingOwner::Track(track.id()), RoutingOwner::Track(parent)));
            }
        }
        for track in &self.tracks {
            for send in track.sends() {
                audio.insert((
                    RoutingOwner::Track(track.id()),
                    RoutingOwner::Return(send.return_track()),
                ));
            }
        }
        for feed in &self.routing {
            audio.insert((
                RoutingOwner::Return(feed.source()),
                RoutingOwner::Return(feed.destination()),
            ));
        }
        let mut authored_audio = BTreeSet::new();
        for feed in &self.audio_feeds {
            self.validate_audio_owner(feed.source)?;
            self.track(feed.destination)
                .ok_or(Invalid("track no longer exists"))?;
            if !feed.gain.is_finite() || !(0.0..=2.0).contains(&feed.gain) || feed.gain == 0.0 {
                return Err(Invalid(
                    "stored audio feed gain must be positive and within 0..2",
                ));
            }
            let edge = (feed.source, RoutingOwner::Track(feed.destination));
            if !authored_audio.insert(edge) {
                return Err(Invalid("duplicate audio feed"));
            }
            // An authored additive feed may parallel the exclusive default
            // output. Preserve both signals; topology needs their union once.
            audio.insert(edge);
        }
        for route in self
            .sidechains
            .iter()
            .filter(|route| Some(route.device) != skip_sidechain)
        {
            audio.insert((
                RoutingOwner::Track(route.source),
                self.sidechain_destination(route.device)?,
            ));
        }
        Ok(audio)
    }
}

fn dag(
    nodes: &BTreeSet<RoutingOwner>,
    edges: &BTreeSet<(RoutingOwner, RoutingOwner)>,
) -> Result<(), Invalid> {
    let mut incoming: BTreeMap<_, usize> = nodes.iter().map(|&owner| (owner, 0)).collect();
    let mut outgoing: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for &(source, destination) in edges {
        if !nodes.contains(&source) {
            return Err(Invalid("routing owner no longer exists"));
        }
        let count = incoming
            .get_mut(&destination)
            .ok_or(Invalid("routing owner no longer exists"))?;
        *count += 1;
        outgoing.entry(source).or_default().push(destination);
    }
    let mut ready: VecDeque<_> = incoming
        .iter()
        .filter_map(|(&owner, &count)| (count == 0).then_some(owner))
        .collect();
    let mut visited = 0;
    while let Some(owner) = ready.pop_front() {
        visited += 1;
        if let Some(destinations) = outgoing.get(&owner) {
            for destination in destinations {
                let count = incoming
                    .get_mut(destination)
                    .ok_or(Invalid("routing owner no longer exists"))?;
                *count -= 1;
                if *count == 0 {
                    ready.push_back(*destination);
                }
            }
        }
    }
    if visited != nodes.len() {
        return Err(Invalid("routing cycles are unsupported"));
    }
    Ok(())
}
