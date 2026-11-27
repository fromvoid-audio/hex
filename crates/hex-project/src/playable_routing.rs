//! Typed, bounded canonical audio inputs, exclusive track outputs, input monitoring,
//! and authored MIDI source selection.
use crate::{Created, Invalid, Project, ReturnId, RouteId, RoutingOwner, Snapshot, TrackId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_AUDIO_FEEDS: usize = crate::MAX_AUTHORED_ROUTES;
pub const MAX_MIDI_ROUTES: usize = crate::MAX_TRACKS;

/// Canonical per-track input monitoring mode (Live 12 §17.1 semantics).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputMonitoring {
    /// Pass the chosen input regardless of arm and suppress the track's clip playback.
    In,
    /// Pass input when armed; inhibit monitoring while the track is playing clips.
    #[default]
    Auto,
    /// Disable software monitoring without deleting source clips or the recording path.
    Off,
}

/// Exclusive post-fader dry output destination for a track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrackOutputDestination {
    /// Route to `parent_group` when inside a group, otherwise to `Main`.
    #[default]
    Default,
    /// Route exclusively to another track's pre-effect stereo input (no Main/group dry path).
    Track(TrackId),
    /// Route exclusively to a Return bus's stereo input (no Main/group dry path).
    Return(ReturnId),
    /// Route directly to Main (even when inside a group).
    Main,
    /// Disconnect direct dry output; post-fader sends and explicit additive feeds remain active.
    SendsOnly,
}

impl TrackOutputDestination {
    /// Construct a Track output destination from a raw identifier for validated snapshot assembly.
    pub fn track_from_raw(id: u64) -> Self {
        Self::Track(TrackId(id))
    }
    /// Construct a Return output destination from a raw identifier for validated snapshot assembly.
    pub fn return_from_raw(id: u64) -> Self {
        Self::Return(ReturnId(id))
    }
}

/// Sparse persisted track I/O configuration entry for schema 129+.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackIoSnapshot {
    pub track: u64,
    pub output: TrackOutputDestination,
    pub monitoring: InputMonitoring,
}

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
    pub fn has_custom_track_io(&self) -> bool {
        self.tracks.iter().any(crate::Track::has_custom_io)
    }

    /// Resolve a track's effective exclusive dry output owner (`None` for `SendsOnly`).
    pub fn effective_track_output(&self, track: TrackId) -> Option<Option<RoutingOwner>> {
        let track = self.track(track)?;
        Some(match track.output_destination() {
            TrackOutputDestination::Default => track
                .parent_group()
                .map(RoutingOwner::Track)
                .or(Some(RoutingOwner::Main)),
            TrackOutputDestination::Track(target) => Some(RoutingOwner::Track(target)),
            TrackOutputDestination::Return(bus) => Some(RoutingOwner::Return(bus)),
            TrackOutputDestination::Main => Some(RoutingOwner::Main),
            TrackOutputDestination::SendsOnly => None,
        })
    }

    /// Eligible non-cyclic stereo audio input sources for `destination`.
    pub fn eligible_audio_input_sources(
        &self,
        destination: TrackId,
    ) -> Result<Vec<RoutingOwner>, Invalid> {
        self.track(destination)
            .ok_or(Invalid("track no longer exists"))?;
        let mut eligible = Vec::new();
        for track in &self.tracks {
            if track.id() == destination {
                continue;
            }
            let owner = RoutingOwner::Track(track.id());
            let mut candidate = self.clone();
            if candidate
                .assign_track_audio_input(destination, Some(owner))
                .is_ok()
            {
                eligible.push(owner);
            }
        }
        for bus in &self.returns {
            let owner = RoutingOwner::Return(bus.id());
            let mut candidate = self.clone();
            if candidate
                .assign_track_audio_input(destination, Some(owner))
                .is_ok()
            {
                eligible.push(owner);
            }
        }
        Ok(eligible)
    }

    /// Eligible non-cyclic MIDI input tracks for `destination` (empty if no instrument).
    pub fn eligible_midi_input_sources(
        &self,
        destination: TrackId,
    ) -> Result<Vec<TrackId>, Invalid> {
        let target = self
            .track(destination)
            .ok_or(Invalid("track no longer exists"))?;
        if target.instrument().is_none() {
            return Ok(Vec::new());
        }
        let mut eligible = Vec::new();
        for track in &self.tracks {
            if track.id() == destination || track.is_group() {
                continue;
            }
            let mut candidate = self.clone();
            if candidate
                .set_midi_source(destination, Some(track.id()))
                .is_ok()
            {
                eligible.push(track.id());
            }
        }
        Ok(eligible)
    }

    /// Eligible non-cyclic exclusive output destinations for `source`.
    pub fn eligible_output_destinations(
        &self,
        source: TrackId,
    ) -> Result<Vec<TrackOutputDestination>, Invalid> {
        self.track(source)
            .ok_or(Invalid("track no longer exists"))?;
        let mut candidates = vec![
            TrackOutputDestination::Default,
            TrackOutputDestination::Main,
            TrackOutputDestination::SendsOnly,
        ];
        for track in &self.tracks {
            if track.id() != source {
                candidates.push(TrackOutputDestination::Track(track.id()));
            }
        }
        for bus in &self.returns {
            candidates.push(TrackOutputDestination::Return(bus.id()));
        }
        let mut eligible = Vec::new();
        for destination in candidates {
            let mut probe = self.clone();
            if probe.set_track_output(source, destination).is_ok() {
                eligible.push(destination);
            }
        }
        Ok(eligible)
    }

    pub(crate) fn set_track_output(
        &mut self,
        track: TrackId,
        destination: TrackOutputDestination,
    ) -> Result<Created, Invalid> {
        self.track(track).ok_or(Invalid("track no longer exists"))?;
        match destination {
            TrackOutputDestination::Default
            | TrackOutputDestination::Main
            | TrackOutputDestination::SendsOnly => {}
            TrackOutputDestination::Track(target) => {
                if target == track {
                    return Err(Invalid("routing cycles are unsupported"));
                }
                self.track(target)
                    .ok_or(Invalid("output destination track no longer exists"))?;
                // Prevent doubled dry signal when an additive feed already connects track -> target.
                self.audio_feeds.retain(|feed| {
                    !(feed.source == RoutingOwner::Track(track) && feed.destination == target)
                });
            }
            TrackOutputDestination::Return(bus) => {
                if !self.returns.iter().any(|candidate| candidate.id() == bus) {
                    return Err(Invalid("output destination return no longer exists"));
                }
            }
        }
        self.track_mut(track)?.output_destination = destination;
        self.validate_playback_routing()?;
        Ok(Created::None)
    }

    pub(crate) fn set_track_input_monitoring(
        &mut self,
        track: TrackId,
        mode: InputMonitoring,
    ) -> Result<Created, Invalid> {
        let target = self.track_mut(track)?;
        if target.is_group() {
            return Err(Invalid("group tracks do not support input monitoring"));
        }
        target.input_monitoring = mode;
        Ok(Created::None)
    }

    /// Atomically assign or clear a track's single selected stereo audio input.
    /// Avoids doubling the signal when `source` already routes its exclusive output to `destination`.
    pub(crate) fn assign_track_audio_input(
        &mut self,
        destination: TrackId,
        source: Option<RoutingOwner>,
    ) -> Result<Created, Invalid> {
        self.track(destination)
            .ok_or(Invalid("track no longer exists"))?;
        if let Some(owner) = source {
            self.validate_audio_owner(owner)?;
            if owner == RoutingOwner::Track(destination) {
                return Err(Invalid("routing cycles are unsupported"));
            }
        }
        self.audio_feeds
            .retain(|feed| feed.destination != destination);
        if let Some(owner) = source {
            let already_exclusive = matches!(
                owner,
                RoutingOwner::Track(src_id)
                    if self.track(src_id).is_some_and(|t| {
                        t.output_destination() == TrackOutputDestination::Track(destination)
                    })
            );
            if !already_exclusive {
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
                    source: owner,
                    destination,
                    gain: 1.0,
                });
            }
        }
        self.validate_playback_routing()?;
        Ok(Created::None)
    }

    pub(crate) fn validate_playable_routes(&self) -> Result<(), Invalid> {
        if self.audio_feeds.is_empty()
            && self.midi_routes.is_empty()
            && self.sidechains.is_empty()
            && self.device_output_routes.is_empty()
            && !self.has_custom_track_io()
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
        // Each track owns at most one effective dry output edge (group, Main,
        // explicit Track, explicit Return, or SendsOnly). Include all of them
        // alongside authored feeds/sends so no cycle can hide across edges.
        for track in &self.tracks {
            match track.output_destination() {
                TrackOutputDestination::Default => {
                    if let Some(parent) = track.parent_group() {
                        audio
                            .insert((RoutingOwner::Track(track.id()), RoutingOwner::Track(parent)));
                    }
                }
                TrackOutputDestination::Track(target) => {
                    if target == track.id() {
                        return Err(Invalid("routing cycles are unsupported"));
                    }
                    self.track(target)
                        .ok_or(Invalid("output destination track no longer exists"))?;
                    audio.insert((RoutingOwner::Track(track.id()), RoutingOwner::Track(target)));
                }
                TrackOutputDestination::Return(bus) => {
                    if !self.returns.iter().any(|candidate| candidate.id() == bus) {
                        return Err(Invalid("output destination return no longer exists"));
                    }
                    audio.insert((RoutingOwner::Track(track.id()), RoutingOwner::Return(bus)));
                }
                TrackOutputDestination::Main => {
                    audio.insert((RoutingOwner::Track(track.id()), RoutingOwner::Main));
                }
                TrackOutputDestination::SendsOnly => {}
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
            if let RoutingOwner::Track(src_id) = feed.source
                && self.track(src_id).is_some_and(|t| {
                    t.output_destination() == TrackOutputDestination::Track(feed.destination)
                })
            {
                return Err(Invalid("track output already feeds this destination"));
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
        for route in &self.device_output_routes {
            let source = self.device_output_source_owner(route.device)?;
            let target = route.destination.into_routing_owner();
            if target == source {
                return Err(Invalid("routing cycles are unsupported"));
            }
            match route.destination {
                crate::DeviceOutputDestination::Track(id) => {
                    self.track(id)
                        .ok_or(Invalid("output destination track no longer exists"))?;
                }
                crate::DeviceOutputDestination::Return(id) => {
                    if !self.returns.iter().any(|bus| bus.id() == id) {
                        return Err(Invalid("output destination return no longer exists"));
                    }
                }
                crate::DeviceOutputDestination::Main => {}
            }
            audio.insert((source, target));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Editor};

    #[test]
    fn audio_feed_rejects_invalid_gain_and_midi_source_resolves_default_track() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(a) = editor
            .execute(Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track a")
        };
        let Created::Track(b) = editor
            .execute(Command::AddTrack { name: "b".into() })
            .unwrap()
        else {
            panic!("track b")
        };
        assert_eq!(editor.project().midi_source_track(a), a);
        assert!(
            editor
                .execute(Command::SetAudioFeed {
                    source: RoutingOwner::Track(a),
                    destination: b,
                    gain: f32::NAN,
                })
                .is_err()
        );
        editor
            .execute(Command::SetAudioFeed {
                source: RoutingOwner::Track(a),
                destination: b,
                gain: 0.75,
            })
            .unwrap();
        assert_eq!(editor.project().audio_feeds().len(), 1);
        assert_eq!(editor.project().audio_feeds()[0].gain(), 0.75);
    }

    #[test]
    fn track_io_output_monitoring_deduplication_cycles_and_cleanup() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(a) = editor
            .execute(Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track a");
        };
        let Created::Track(b) = editor
            .execute(Command::AddTrack { name: "b".into() })
            .unwrap()
        else {
            panic!("track b");
        };
        let Created::ReturnTrack(ret) = editor
            .execute(Command::AddReturnTrack { name: "rev".into() })
            .unwrap()
        else {
            panic!("return");
        };

        // Assign audio input A -> B, then set A output -> B: must remove duplicate feed atomically!
        editor
            .execute(Command::AssignTrackAudioInput {
                destination: b,
                source: Some(RoutingOwner::Track(a)),
            })
            .unwrap();
        assert_eq!(editor.project().audio_feeds().len(), 1);
        editor
            .execute(Command::SetTrackOutput {
                track: a,
                destination: TrackOutputDestination::Track(b),
            })
            .unwrap();
        assert_eq!(
            editor.project().track(a).unwrap().output_destination(),
            TrackOutputDestination::Track(b)
        );
        assert!(
            editor.project().audio_feeds().is_empty(),
            "exclusive track output must deduplicate parallel additive feed"
        );

        // Assigning B's audio input to A again must NOT re-add a duplicate feed!
        editor
            .execute(Command::AssignTrackAudioInput {
                destination: b,
                source: Some(RoutingOwner::Track(a)),
            })
            .unwrap();
        assert!(editor.project().audio_feeds().is_empty());

        // Reverse output B -> A forms a cycle and must be refused atomically without mutating state.
        let before = editor.project().clone();
        assert!(
            editor
                .execute(Command::SetTrackOutput {
                    track: b,
                    destination: TrackOutputDestination::Track(a),
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);

        // Self-output is refused.
        assert!(
            editor
                .execute(Command::SetTrackOutput {
                    track: a,
                    destination: TrackOutputDestination::Track(a),
                })
                .is_err()
        );

        // Combined cycle via Return: B -> Return -> audio feed into A is refused.
        editor
            .execute(Command::SetTrackOutput {
                track: b,
                destination: TrackOutputDestination::Return(ret),
            })
            .unwrap();
        assert!(
            editor
                .execute(Command::AssignTrackAudioInput {
                    destination: a,
                    source: Some(RoutingOwner::Return(ret)),
                })
                .is_err()
        );

        // Input monitoring modes and Undo/Redo round-trip.
        editor
            .execute(Command::SetTrackInputMonitoring {
                track: b,
                mode: InputMonitoring::In,
            })
            .unwrap();
        assert_eq!(
            editor.project().track(b).unwrap().input_monitoring(),
            InputMonitoring::In
        );
        assert!(editor.undo());
        assert_eq!(
            editor.project().track(b).unwrap().input_monitoring(),
            InputMonitoring::Auto
        );
        assert!(editor.redo());
        assert_eq!(
            editor.project().track(b).unwrap().input_monitoring(),
            InputMonitoring::In
        );

        // Deleting destination return/track resets dependent output destinations to Default.
        editor.execute(Command::RemoveReturnTrack(ret)).unwrap();
        assert_eq!(
            editor.project().track(b).unwrap().output_destination(),
            TrackOutputDestination::Default
        );
        editor.execute(Command::RemoveTrack(b)).unwrap();
        assert_eq!(
            editor.project().track(a).unwrap().output_destination(),
            TrackOutputDestination::Default
        );
    }
}
