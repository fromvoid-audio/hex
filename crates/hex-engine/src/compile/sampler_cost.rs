//! Only the closed original-Sampler/no-audio/no-preview compiler class.
//! Constructor sizes, not an input union or permission to construct.
use super::*;
#[derive(Clone, Copy)]
pub(crate) struct Cost {
    pub(crate) retained: usize,
    pub(crate) peak: usize,
}
fn add(a: usize, b: usize) -> Result<usize, Invalid> {
    a.checked_add(b)
        .ok_or(Invalid("sampler compiler cost overflow"))
}
fn bytes<T>(n: usize) -> Result<usize, Invalid> {
    std::alloc::Layout::array::<T>(n).map_err(|_| Invalid("sampler compiler cost overflow"))?;
    n.checked_mul(size_of::<T>())
        .ok_or(Invalid("sampler compiler cost overflow"))
}
pub(crate) fn quote(project: &hex_project::Project) -> Result<Cost, Invalid> {
    if !crate::audio_storage_constructor_layout_supported()
        || project
            .tracks()
            .iter()
            .any(|track| !track.clips().is_empty())
        || !project.audio_feeds().is_empty()
    {
        return Err(Invalid("sampler compiler class is unsupported"));
    }
    crate::gain_envelope::validate_project(project)?;
    let feed = crate::return_feed_plan::ReturnFeedPlan::construction_cost(project)?;
    let graph = crate::signal_graph::SignalGraph::construction_cost(project)?;
    let tempo = project
        .tempo_map()
        .owned_allocation_layout()
        .checked_backing_bytes()
        .ok_or(Invalid("sampler tempo copy cost overflow"))?;
    // The freshly assembled Project has empty Vecs, regardless of spare root
    // backings. Their old capacities stay in the actual input inventory.
    let products = add(
        add(
            bytes::<crate::Track>(project.tracks().len())?,
            bytes::<crate::ReturnBus>(project.return_tracks().len())?,
        )?,
        add(
            tempo,
            bytes::<(hex_project::TrackId, hex_project::TrackId)>(project.midi_routes().len())?,
        )?,
    )?;
    let mut result = Cost {
        retained: add(products, add(feed.retained_bytes, graph.retained_bytes)?)?,
        peak: add(
            products,
            add(feed.construction_peak_bytes, graph.construction_peak_bytes)?,
        )?,
    };
    // This temporary compiler feed list coexists with the fresh active plan
    // and the persistent paired Return plan; neither stands in for this Vec.
    result.peak = add(
        result.peak,
        bytes::<(usize, usize, f32)>(project.authored_routing().len())?,
    )?;
    for destination in project.tracks() {
        let source = project
            .track(project.midi_source_track(destination.id()))
            .ok_or(Invalid("sampler midi source root is missing"))?;
        let audible = destination
            .instrument()
            .is_some_and(|device| !device.bypassed());
        let midi = crate::midi_compile::construction::quote(source, audible)?;
        let gain = crate::gain_envelope::PreparedGainEnvelope::construction_cost(
            destination.gain_lane(),
            project.tempo_map(),
        )?;
        result.retained = add(
            result.retained,
            add(midi.retained_bytes, gain.backing_bytes)?,
        )?;
        result.peak = add(
            result.peak,
            add(midi.construction_peak_bytes, gain.backing_bytes)?,
        )?;
    }
    Ok(result)
}
