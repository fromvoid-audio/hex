//! Private canonical-model routing validation. No snapshots or model owners are cloned.
//! The scratch quote covers this helper only; it is not a native admission permit.
use crate::Invalid;
use hex_project::{Project, RoutingOwner};

#[derive(Clone, Copy)]
struct Counts {
    nodes: usize,
    audio: usize,
    midi: usize,
    ids: usize,
}
impl Counts {
    fn of(project: &Project) -> Result<Self, Invalid> {
        if project.tracks().len() > hex_project::MAX_TRACKS
            || project.return_tracks().len() > hex_project::MAX_RETURN_TRACKS
            || project.authored_routing().len() > hex_project::MAX_RETURN_FEEDS
            || project.audio_feeds().len() > hex_project::MAX_AUDIO_FEEDS
            || project.midi_routes().len() > hex_project::MAX_MIDI_ROUTES
        {
            return Err(Invalid("authored routing edge limit reached"));
        }
        let owners = add(project.tracks().len(), project.return_tracks().len())?;
        let mut audio = add(owners, project.authored_routing().len())?;
        audio = add(audio, project.audio_feeds().len())?;
        for track in project.tracks() {
            if track.sends().len() > hex_project::MAX_RETURN_SENDS_PER_TRACK {
                return Err(Invalid("return send limit reached"));
            }
            audio = add(audio, track.sends().len())?;
        }
        Ok(Self {
            nodes: add(owners, 1)?,
            audio,
            midi: project.midi_routes().len(),
            ids: add(
                add(
                    project.authored_routing().len(),
                    project.audio_feeds().len(),
                )?,
                project.midi_routes().len(),
            )?,
        })
    }
    fn bytes(self) -> Result<usize, Invalid> {
        let mut bytes = size_of::<Scratch>();
        for (count, size) in [
            (self.nodes, size_of::<RoutingOwner>()),
            (self.audio, size_of::<(usize, usize)>()),
            (self.midi, size_of::<(usize, usize)>()),
            (self.nodes, size_of::<usize>()),
            (self.nodes, size_of::<bool>()),
            (self.ids, size_of::<u64>()),
        ] {
            bytes = add(
                bytes,
                count
                    .checked_mul(size)
                    .ok_or(Invalid("routing scratch overflow"))?,
            )?;
        }
        Ok(bytes)
    }
}
fn add(a: usize, b: usize) -> Result<usize, Invalid> {
    a.checked_add(b).ok_or(Invalid("routing scratch overflow"))
}
#[derive(Default)]
struct Scratch {
    nodes: Vec<RoutingOwner>,
    audio: Vec<(usize, usize)>,
    midi: Vec<(usize, usize)>,
    incoming: Vec<usize>,
    visited: Vec<bool>,
    ids: Vec<u64>,
}
fn reserved<T>(count: usize) -> Result<Vec<T>, Invalid> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|_| Invalid("routing scratch allocation failed"))?;
    Ok(result)
}
impl Scratch {
    fn new(counts: Counts, limit: usize) -> Result<Self, Invalid> {
        // Refuse the requested construction before the first allocation.
        if counts.bytes()? > limit {
            return Err(Invalid("routing scratch reservation exceeded"));
        }
        let result = Self {
            nodes: reserved(counts.nodes)?,
            audio: reserved(counts.audio)?,
            midi: reserved(counts.midi)?,
            incoming: reserved(counts.nodes)?,
            visited: reserved(counts.nodes)?,
            ids: reserved(counts.ids)?,
        };
        // Actual capacities are checked, not assumed to equal reserve requests.
        let mut bytes = size_of::<Scratch>();
        for (capacity, size) in [
            (result.nodes.capacity(), size_of::<RoutingOwner>()),
            (result.audio.capacity(), size_of::<(usize, usize)>()),
            (result.midi.capacity(), size_of::<(usize, usize)>()),
            (result.incoming.capacity(), size_of::<usize>()),
            (result.visited.capacity(), size_of::<bool>()),
            (result.ids.capacity(), size_of::<u64>()),
        ] {
            bytes = add(
                bytes,
                capacity
                    .checked_mul(size)
                    .ok_or(Invalid("routing scratch overflow"))?,
            )?;
        }
        if bytes > limit {
            return Err(Invalid("routing scratch reservation exceeded"));
        }
        Ok(result)
    }
    fn node(&self, owner: RoutingOwner) -> Result<usize, Invalid> {
        self.nodes
            .iter()
            .position(|&node| node == owner)
            .ok_or(Invalid("routing owner no longer exists"))
    }
    fn id(&mut self, id: u64, project: &Project) -> Result<(), Invalid> {
        if id == 0 || id >= project.next_identity() {
            return Err(Invalid("invalid routing identity"));
        }
        self.ids.push(id);
        Ok(())
    }
    fn dag(&mut self, midi: bool) -> Result<(), Invalid> {
        let edges = if midi {
            &mut self.midi
        } else {
            &mut self.audio
        };
        edges.sort_unstable();
        if let Some(pair) = edges.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(Invalid(match self.nodes[pair[0].1] {
                RoutingOwner::Return(_) => "duplicate return feed",
                _ => "duplicate audio feed",
            }));
        }
        self.incoming.clear();
        self.incoming.resize(self.nodes.len(), 0);
        self.visited.clear();
        self.visited.resize(self.nodes.len(), false);
        for &(_, destination) in edges.iter() {
            self.incoming[destination] += 1;
        }
        for _ in 0..self.nodes.len() {
            let source = (0..self.nodes.len())
                .find(|&i| !self.visited[i] && self.incoming[i] == 0)
                .ok_or(Invalid("routing cycles are unsupported"))?;
            self.visited[source] = true;
            let start = edges.partition_point(|&(s, _)| s < source);
            for &(_, destination) in edges[start..].iter().take_while(|&&(s, _)| s == source) {
                self.incoming[destination] -= 1;
            }
        }
        Ok(())
    }
}
fn gain(value: f32) -> Result<(), Invalid> {
    if !value.is_finite() || value <= 0.0 || value > 2.0 {
        return Err(Invalid(
            "stored routing gain must be positive and within 0..2",
        ));
    }
    Ok(())
}
// Requested scratch bound from the same scalar counts as the constructor.
// No allocation, owning clone or routing-admission authority is returned.
pub(super) fn construction_scratch_bytes(project: &Project) -> Result<usize, Invalid> {
    Counts::of(project)?.bytes()
}

pub(super) fn validate(project: &Project) -> Result<(), Invalid> {
    let counts = Counts::of(project)?;
    validate_reserved(project, counts, counts.bytes()?)
}
fn validate_reserved(project: &Project, counts: Counts, limit: usize) -> Result<(), Invalid> {
    let mut scratch = Scratch::new(counts, limit)?;
    scratch
        .nodes
        .extend(project.tracks().iter().map(|t| RoutingOwner::Track(t.id())));
    scratch.nodes.extend(
        project
            .return_tracks()
            .iter()
            .map(|t| RoutingOwner::Return(t.id())),
    );
    scratch.nodes.push(RoutingOwner::Main);
    let main = scratch.nodes.len() - 1;
    for source in 0..main {
        scratch.audio.push((source, main));
    }
    for track in project.tracks() {
        let source = scratch.node(RoutingOwner::Track(track.id()))?;
        for send in track.sends() {
            gain(send.gain())?;
            let destination = scratch.node(RoutingOwner::Return(send.return_track()))?;
            scratch.audio.push((source, destination));
        }
    }
    for feed in project.authored_routing() {
        gain(feed.gain())?;
        scratch.id(feed.id().value(), project)?;
        let source = scratch.node(RoutingOwner::Return(feed.source()))?;
        let destination = scratch.node(RoutingOwner::Return(feed.destination()))?;
        scratch.audio.push((source, destination));
    }
    for feed in project.audio_feeds() {
        gain(feed.gain())?;
        scratch.id(feed.id().value(), project)?;
        if feed.source() == RoutingOwner::Main {
            return Err(Invalid("main cannot feed a track"));
        }
        let source = scratch.node(feed.source())?;
        let destination = scratch.node(RoutingOwner::Track(feed.destination()))?;
        scratch.audio.push((source, destination));
    }
    for route in project.midi_routes() {
        scratch.id(route.id().value(), project)?;
        let source = scratch.node(RoutingOwner::Track(route.source()))?;
        let destination = scratch.node(RoutingOwner::Track(route.destination()))?;
        if project
            .track(route.destination())
            .and_then(|t| t.instrument())
            .is_none()
        {
            return Err(Invalid("midi destination requires an instrument"));
        }
        if scratch.midi.iter().any(|&(_, d)| d == destination) {
            return Err(Invalid("duplicate midi destination"));
        }
        scratch.midi.push((source, destination));
    }
    scratch.ids.sort_unstable();
    if scratch.ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Invalid("duplicate routing identity"));
    }
    scratch.dag(false)?;
    scratch.dag(true)
}

#[cfg(test)]
mod tests;
