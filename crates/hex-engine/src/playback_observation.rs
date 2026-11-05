//! Exact read-only observations copied from one compiled revision on control. These values own
//! only scalar MIDI gates/indexes/timing and existing scalar-atomic meter endpoints. They contain no
//! PCM, prepared revision, instrument/effect factory, native configuration or analytic context.
//! Consumers must correlate them with the actual applied output/context/audio revision.
use crate::midi_compile::{CompiledNoteSpan, SpanNode};
use crate::midi_template::PreparedMidiGateTemplate;
use crate::prepared_tempo::{
    AnchoredSessionTempoView, PreparedPeriodicSessionTempo, PreparedTempoMap,
    SessionBaseTimingView, SessionTimingView,
};
use crate::{
    Invalid, MidiSpanBuffer, PreparedRevision, SessionClockAnchor, SessionMidiOrigin,
    SessionPlaybackOwner,
};
use hex_project::{BeatTime, ClipId, TrackId};
use std::sync::Arc;

#[derive(Debug)]
struct Meter {
    id: u64,
    kind: hex_lattice_contract::DeviceTypeId,
    meter: Arc<hex_lattice::DeviceMeter>,
    foreign: bool,
}
#[derive(Debug)]
struct TrackGates {
    track: TrackId,
    output_meter: Arc<hex_lattice::DeviceMeter>,
    route: TrackId,
    spans: Vec<CompiledNoteSpan>,
    nodes: Vec<SpanNode>,
    root: Option<usize>,
    has_instrument: bool,
}
#[derive(Debug)]
struct ReturnMeter {
    id: hex_project::ReturnId,
    meter: Arc<hex_lattice::DeviceMeter>,
}
#[derive(Debug)]
struct SessionGates {
    track: TrackId,
    clip: ClipId,
    enabled: bool,
    gates: PreparedMidiGateTemplate,
}

/// Resource-free exact gate schedules, including routed Session MIDI and periodic tempo.
/// Every admitted track is represented, including audio/group/noninstrument tracks. Queries use
/// the playback reporter and existing fixed span buffer/work bounds, never sampled drawing data.
#[derive(Debug)]
pub struct PreparedMidiActivity {
    tracks: Vec<TrackGates>,
    session: Vec<SessionGates>,
    timing: PreparedTempoMap,
    periodic: Option<PreparedPeriodicSessionTempo>,
    tempo_unavailable: bool,
}
/// Bounded read-only telemetry for one actual submitted graph. Holding this projection cannot
/// extend original PCM/configuration/factory lifetime. Control retains and charges an anchor
/// independently of graph return until all external projection copies have physically retired.
#[derive(Debug)]
pub struct PreparedPlaybackObservation {
    meters: Vec<Meter>,
    returns: Vec<ReturnMeter>,
    activity: PreparedMidiActivity,
}

impl PreparedPlaybackObservation {
    /// Control-only opt-in for Track/Return render-work sampling. These are the
    /// existing scalar endpoints, not new graph/processor/resource owners.
    /// Only enable an observation correlated with the applied output/revision.
    pub fn set_render_work_enabled(&self, enabled: bool) {
        for (_, meter) in self.render_work_meters() {
            meter.set_enabled(enabled);
        }
    }

    /// Stable canonical identities for sampled local audio-thread work. A
    /// missing sample is unknown/disabled, not zero. Main, detached historical
    /// tails and other shared costs remain in the global callback meter.
    pub fn render_work_meters(
        &self,
    ) -> impl Iterator<Item = (crate::OutputCaptureId, &hex_lattice::RenderWorkMeter)> {
        self.activity
            .tracks
            .iter()
            .map(|track| {
                (
                    crate::OutputCaptureId::Track(track.track),
                    track.output_meter.render_work(),
                )
            })
            .chain(self.returns.iter().map(|bus| {
                (
                    crate::OutputCaptureId::Return(bus.id),
                    bus.meter.render_work(),
                )
            }))
    }

    /// Finite requested construction storage before allocation, including the outer Arc header
    /// and conservative backing for shared meter endpoints. Actual capacities are checked after
    /// capture. This is control metadata accounting, not allocator/RSS or graph provenance.
    pub fn preparation_storage_quote_on_control(
        revision: &PreparedRevision,
    ) -> Result<usize, Invalid> {
        Self::storage_quote(
            revision,
            crate::session_inventory::EffectiveSessionMidi::new(revision.session(), None),
        )
    }
    pub(crate) fn storage_quote(
        revision: &PreparedRevision,
        midi: crate::session_inventory::EffectiveSessionMidi<'_>,
    ) -> Result<usize, Invalid> {
        let count = meter_sources(revision).count();
        if count > hex_project::MAX_DEVICES {
            return Err(Invalid("observation device count exceeds its limit"));
        }
        let mut bytes = std::mem::size_of::<Self>()
            .checked_add(2 * std::mem::size_of::<usize>())
            .and_then(|n| {
                n.checked_add(
                    count.checked_mul(std::mem::size_of::<Meter>() + meter_backing_bytes())?,
                )
            })
            .ok_or(Invalid("observation storage overflow"))?;
        add(
            &mut bytes,
            revision
                .arrangement()
                .return_buses()
                .len()
                .checked_mul(std::mem::size_of::<ReturnMeter>() + meter_backing_bytes()),
        )?;
        add(
            &mut bytes,
            revision.session().timing().observation_copy_quote(),
        )?;
        add(
            &mut bytes,
            revision
                .arrangement()
                .tracks()
                .len()
                .checked_mul(std::mem::size_of::<TrackGates>() + meter_backing_bytes()),
        )?;
        for track in revision.arrangement().tracks() {
            if let Some(source) = &track.instrument {
                add(
                    &mut bytes,
                    source
                        .spans
                        .len()
                        .checked_mul(std::mem::size_of::<CompiledNoteSpan>()),
                )?;
                add(
                    &mut bytes,
                    source
                        .nodes
                        .len()
                        .checked_mul(std::mem::size_of::<SpanNode>()),
                )?;
            }
        }
        for clip in midi.activity_midi_clips() {
            add(&mut bytes, Some(std::mem::size_of::<SessionGates>()))?;
            add(
                &mut bytes,
                clip.instrument_template()
                    .gate_capture_quote()
                    .and_then(|n| n.checked_sub(std::mem::size_of::<PreparedMidiGateTemplate>())),
            )?;
        }
        Ok(bytes)
    }
    pub fn prepare_on_control(revision: &PreparedRevision) -> Result<Self, Invalid> {
        Self::prepare_effective(
            revision,
            crate::session_inventory::EffectiveSessionMidi::new(revision.session(), None),
            revision.session().periodic_tempo(),
            revision.session().periodic_tempo_error().is_some(),
        )
    }
    pub(crate) fn prepare_effective(
        revision: &PreparedRevision,
        midi: crate::session_inventory::EffectiveSessionMidi<'_>,
        periodic: Option<PreparedPeriodicSessionTempo>,
        tempo_unavailable: bool,
    ) -> Result<Self, Invalid> {
        let quote = Self::storage_quote(revision, midi)?;
        let mut meters = reserved(meter_sources(revision).count())?;
        for (id, kind, meter, foreign) in meter_sources(revision) {
            meters.push(Meter {
                id,
                kind,
                meter: Arc::clone(meter),
                foreign,
            });
        }
        meters.sort_unstable_by_key(|meter| meter.id);
        if meters.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(Invalid("observation device identity is duplicated"));
        }
        let mut tracks = reserved(revision.arrangement().tracks().len())?;
        let mut returns = reserved(revision.arrangement().return_buses().len())?;
        for bus in revision.arrangement().return_buses() {
            returns.push(ReturnMeter {
                id: bus.id,
                meter: Arc::clone(&bus.output_meter),
            });
        }
        for track in revision.arrangement().tracks() {
            let id = track
                .canonical_track_id()
                .ok_or(Invalid("observation track identity unavailable"))?;
            let (spans, nodes, root) = match &track.instrument {
                Some(source) => (copy(&source.spans)?, copy(&source.nodes)?, source.root),
                None => (Vec::new(), Vec::new(), None),
            };
            tracks.push(TrackGates {
                track: id,
                output_meter: Arc::clone(&track.instrument_meter),
                route: revision.midi_activity_route_track(id)?,
                spans,
                nodes,
                root,
                has_instrument: track.instrument.is_some(),
            });
        }
        let mut session = reserved(midi.activity_midi_clips().count())?;
        for clip in midi.activity_midi_clips() {
            session.push(SessionGates {
                track: clip.track,
                clip: clip.id,
                enabled: clip.instrument_enabled(),
                gates: clip
                    .instrument_template()
                    .capture_gate_template()
                    .map_err(|_| Invalid("could not capture activity gates"))?,
            });
        }
        let result = Self {
            meters,
            returns,
            activity: PreparedMidiActivity {
                tracks,
                session,
                timing: revision.session().timing().observation_copy()?,
                periodic,
                tempo_unavailable,
            },
        };
        if result
            .retained_bytes_on_control()
            .is_none_or(|bytes| bytes > quote)
        {
            return Err(Invalid(
                "observation allocation exceeds its construction quote",
            ));
        }
        Ok(result)
    }
    /// Borrow an endpoint without allowing its owner to escape the charged observation.
    pub fn meter(
        &self,
        device_id: u64,
        device_type: hex_lattice_contract::DeviceTypeId,
    ) -> Option<&hex_lattice::DeviceMeter> {
        self.meters
            .binary_search_by_key(&device_id, |meter| meter.id)
            .ok()
            .and_then(|index| self.meters.get(index))
            .filter(|meter| meter.kind == device_type)
            .map(|meter| meter.meter.as_ref())
    }
    pub fn activity(&self) -> &PreparedMidiActivity {
        &self.activity
    }
    /// Borrow the independently drained post-fader peak for a canonical track.
    /// The caller must match this observation to the applied audio revision.
    pub fn track_output_meter(&self, track: TrackId) -> Option<&hex_lattice::DeviceMeter> {
        self.activity
            .tracks
            .iter()
            .find(|item| item.track == track)
            .map(|item| item.output_meter.as_ref())
    }
    /// Borrow a return's independently drained post-fader peak.
    pub fn return_output_meter(
        &self,
        id: hex_project::ReturnId,
    ) -> Option<&hex_lattice::DeviceMeter> {
        self.returns
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.meter.as_ref())
    }
    /// Whether this exact compiled device selected a foreign processor. This
    /// scalar is not native admission or ACK: consumers must first match this
    /// observation to the actual applied output/context/audio revision.
    pub fn foreign_processor(
        &self,
        device_id: u64,
        device_type: hex_lattice_contract::DeviceTypeId,
    ) -> Option<bool> {
        self.meters
            .binary_search_by_key(&device_id, |meter| meter.id)
            .ok()
            .and_then(|index| self.meters.get(index))
            .filter(|meter| meter.kind == device_type)
            .map(|meter| meter.foreign)
    }
    pub fn retained_bytes_on_control(&self) -> Option<usize> {
        let mut bytes = std::mem::size_of::<Self>()
            .checked_add(2 * std::mem::size_of::<usize>())?
            .checked_add(
                self.meters
                    .capacity()
                    .checked_mul(std::mem::size_of::<Meter>())?,
            )?
            .checked_add(self.meters.len().checked_mul(meter_backing_bytes())?)?
            .checked_add(
                self.returns
                    .capacity()
                    .checked_mul(std::mem::size_of::<ReturnMeter>())?,
            )?
            .checked_add(self.returns.len().checked_mul(meter_backing_bytes())?)?
            .checked_add(
                self.activity
                    .tracks
                    .capacity()
                    .checked_mul(std::mem::size_of::<TrackGates>())?,
            )?
            .checked_add(
                self.activity
                    .tracks
                    .len()
                    .checked_mul(meter_backing_bytes())?,
            )?
            .checked_add(
                self.activity
                    .session
                    .capacity()
                    .checked_mul(std::mem::size_of::<SessionGates>())?,
            )?
            .checked_add(self.activity.timing.observation_heap_bytes()?)?;
        for track in &self.activity.tracks {
            bytes = bytes
                .checked_add(
                    track
                        .spans
                        .capacity()
                        .checked_mul(std::mem::size_of::<CompiledNoteSpan>())?,
                )?
                .checked_add(
                    track
                        .nodes
                        .capacity()
                        .checked_mul(std::mem::size_of::<SpanNode>())?,
                )?;
        }
        for clip in &self.activity.session {
            bytes = bytes.checked_add(
                clip.gates
                    .storage_bytes()?
                    .checked_sub(std::mem::size_of::<PreparedMidiGateTemplate>())?,
            )?;
        }
        Some(bytes)
    }
}
impl PreparedMidiActivity {
    pub fn route_track(&self, track: TrackId) -> Result<TrackId, Invalid> {
        self.tracks
            .iter()
            .find(|candidate| candidate.track == track)
            .map(|candidate| candidate.route)
            .ok_or(Invalid("midi activity track is unavailable"))
    }
    pub fn session_midi_clip(&self, track: TrackId, clip_id: u64) -> Option<ClipId> {
        self.session
            .iter()
            .find(|clip| clip.track == track && clip.clip.value() == clip_id)
            .map(|clip| clip.clip)
    }
    /// Resolve an accepted performance cursor on the exact Session clock,
    /// including periodic tempo and the applied runtime's revision translation.
    pub fn performance_beat_at(
        &self,
        frame: u64,
        clock_anchor: Option<SessionClockAnchor>,
    ) -> Result<BeatTime, Invalid> {
        use crate::MidiTiming;
        self.activity_timing(clock_anchor)?
            .frame_to_beat(frame)
            .map_err(|_| Invalid("midi activity performance beat is unavailable"))
    }

    fn activity_timing(
        &self,
        clock_anchor: Option<SessionClockAnchor>,
    ) -> Result<SessionTimingView<'_>, Invalid> {
        if self.tempo_unavailable {
            return Err(Invalid("midi activity tempo is unavailable"));
        }
        let base = self
            .periodic
            .map_or(SessionBaseTimingView::Canonical(&self.timing), |periodic| {
                SessionBaseTimingView::Periodic(periodic.view(&self.timing))
            });
        Ok(clock_anchor.map_or_else(
            || SessionTimingView::from_base(base),
            |anchor| SessionTimingView::Anchored(AnchoredSessionTempoView::new(base, anchor)),
        ))
    }

    pub fn gate_keys(
        &self,
        track: TrackId,
        owner: SessionPlaybackOwner,
        origin: Option<SessionMidiOrigin>,
        clock_anchor: Option<SessionClockAnchor>,
        cursors: (u64, u64),
        spans: &mut MidiSpanBuffer,
    ) -> Result<[u64; 2], Invalid> {
        let mut keys = [0; 2];
        let mut mark = |pitch: u8| {
            keys[usize::from(pitch) / 64] |= 1u64 << (pitch % 64);
        };
        match owner {
            SessionPlaybackOwner::Arrangement => {
                let source = self
                    .tracks
                    .iter()
                    .find(|source| source.track == track)
                    .ok_or(Invalid("midi activity track is unavailable"))?;
                if !source.has_instrument {
                    return Ok(keys);
                }
                let end = cursors
                    .0
                    .checked_add(1)
                    .ok_or(Invalid("midi activity frame exhausted"))?;
                crate::midi_activity::visit_arrangement_spans(
                    &source.spans,
                    &source.nodes,
                    source.root,
                    cursors.0,
                    end,
                    None,
                    |span| {
                        if span.start <= cursors.0 && cursors.0 < span.gate_end {
                            mark(span.pitch);
                        }
                    },
                );
            }
            SessionPlaybackOwner::Session { clip } => {
                let source = self
                    .session
                    .iter()
                    .find(|source| source.track == track && source.clip == clip)
                    .ok_or(Invalid("midi activity source is unavailable"))?;
                if source.enabled {
                    let origin = origin.ok_or(Invalid("midi activity origin is unavailable"))?;
                    let timing = self.activity_timing(clock_anchor)?;
                    source
                        .gates
                        .query_with_origin(
                            origin,
                            &timing,
                            cursors.1,
                            cursors
                                .1
                                .checked_add(1)
                                .ok_or(Invalid("midi activity frame exhausted"))?,
                            spans,
                        )
                        .map_err(|_| Invalid("midi activity query was refused"))?;
                    for span in spans.spans() {
                        if span.start_frame <= cursors.1 && cursors.1 < span.gate_end_frame {
                            mark(span.pitch);
                        }
                    }
                }
            }
            SessionPlaybackOwner::Silence => {}
        }
        Ok(keys)
    }
}
fn meter_backing_bytes() -> usize {
    std::mem::size_of::<hex_lattice::DeviceMeter>() + 2 * std::mem::size_of::<usize>()
}
fn meter_sources(
    revision: &PreparedRevision,
) -> impl Iterator<
    Item = (
        u64,
        hex_lattice_contract::DeviceTypeId,
        &Arc<hex_lattice::DeviceMeter>,
        bool,
    ),
> + Clone {
    revision
        .arrangement()
        .tracks()
        .iter()
        .flat_map(|track| {
            track
                .instrument_id
                .zip(track.instrument_state.as_ref())
                .map(|(id, state)| (id.value(), state.type_id(), &track.instrument_meter, false))
                .into_iter()
                .chain(track.device_chain.iter().map(|device| {
                    (
                        device.device_id,
                        device.device_type,
                        &device.meter,
                        matches!(
                            device.definition.as_ref(),
                            Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
                        ),
                    )
                }))
        })
        .chain(
            revision
                .arrangement()
                .return_buses()
                .iter()
                .flat_map(|bus| bus.device_chain.iter())
                .chain(revision.arrangement().main_device_chain.iter())
                .map(|device| {
                    (
                        device.device_id,
                        device.device_type,
                        &device.meter,
                        matches!(
                            device.definition.as_ref(),
                            Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
                        ),
                    )
                }),
        )
}
fn add(total: &mut usize, bytes: Option<usize>) -> Result<(), Invalid> {
    *total = total
        .checked_add(bytes.ok_or(Invalid("observation storage overflow"))?)
        .ok_or(Invalid("observation storage overflow"))?;
    Ok(())
}
fn reserved<T>(count: usize) -> Result<Vec<T>, Invalid> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve observation storage"))?;
    Ok(result)
}
fn copy<T: Clone>(source: &[T]) -> Result<Vec<T>, Invalid> {
    let mut result = reserved(source.len())?;
    result.extend_from_slice(source);
    Ok(result)
}
