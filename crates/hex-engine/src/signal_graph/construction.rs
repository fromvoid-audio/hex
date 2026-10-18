//! Private conservative canonical topology construction cost, including None.
//! Actual prepared fused chains may be smaller. No graph or provider is built.
use super::*;
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConstructionCost {
    pub(crate) retained_bytes: usize,
    pub(crate) construction_peak_bytes: usize,
}
fn sum(total: &mut usize, value: usize) -> Result<(), Invalid> {
    *total = total
        .checked_add(value)
        .ok_or(Invalid("signal graph cost overflow"))?;
    Ok(())
}
fn bytes<T>(count: usize) -> Result<usize, Invalid> {
    std::alloc::Layout::array::<T>(count).map_err(|_| Invalid("signal graph cost overflow"))?;
    count
        .checked_mul(size_of::<T>())
        .ok_or(Invalid("signal graph cost overflow"))
}
impl SignalGraph {
    pub(crate) fn construction_cost(
        canonical: &hex_project::Project,
    ) -> Result<ConstructionCost, Invalid> {
        if !crate::audio_storage_constructor_layout_supported() {
            return Err(Invalid("signal graph constructor layout is unsupported"));
        }
        let mut nodes = 1usize;
        let mut edges = canonical
            .authored_routing()
            .len()
            .checked_add(canonical.audio_feeds().len())
            .ok_or(Invalid("signal graph cost overflow"))?;
        let mut nested = bytes::<Send>(edges)?;
        for track in canonical.tracks() {
            let count = 4usize
                .checked_add(usize::from(
                    track.instrument().is_some_and(|device| !device.bypassed()),
                ))
                .and_then(|n| n.checked_add(track.devices().len()))
                .ok_or(Invalid("signal graph cost overflow"))?;
            sum(&mut nodes, count)?;
            sum(&mut edges, count)?;
            sum(&mut edges, track.sends().len())?;
            sum(&mut nested, bytes::<usize>(track.devices().len())?)?;
            sum(&mut nested, bytes::<Send>(track.sends().len())?)?;
        }
        for bus in canonical.return_tracks() {
            let count = 2usize
                .checked_add(bus.devices().len())
                .ok_or(Invalid("signal graph cost overflow"))?;
            sum(&mut nodes, count)?;
            sum(&mut edges, count)?;
            sum(&mut nested, bytes::<usize>(bus.devices().len())?)?;
        }
        // Canonical counts are conservative under ordinary effect fusion.
        // Existing prepared counts enforce graph policy; a larger upper bound
        // here must not reject a smaller otherwise-supported legacy graph.
        let owners = canonical
            .tracks()
            .len()
            .checked_add(canonical.return_tracks().len())
            .ok_or(Invalid("signal graph cost overflow"))?;
        let (body, header) = OwnedShared::<SignalGraph>::declared_parts()
            .ok_or(Invalid("signal graph constructor layout is unsupported"))?;
        let mut retained = body
            .checked_add(header)
            .ok_or(Invalid("signal graph cost overflow"))?;
        for size in [
            bytes::<TrackPlan>(canonical.tracks().len())?,
            bytes::<ReturnPlan>(canonical.return_tracks().len())?,
            bytes::<Owner>(owners)?,
            bytes::<NodeId>(nodes)?,
            bytes::<PlannedNode>(nodes)?,
            bytes::<PlannedEdge>(edges)?,
            bytes::<usize>(nodes)?, // final topological order
            nested,
        ] {
            sum(&mut retained, size)?;
        }
        let mut peak = retained;
        // Sum simultaneous and sequential scratch conservatively rather than
        // claiming a sampled allocator peak or assuming early local destruction.
        for size in [
            bytes::<(NodeId, Option<u32>)>(nodes)?, // Builder nodes
            bytes::<(usize, usize)>(edges)?,        // Builder edges
            bytes::<LatencyNode>(nodes)?,
            bytes::<LatencyEdge>(edges)?,
            bytes::<u64>(edges)?,   // stable edge mapping
            bytes::<usize>(nodes)?, // LatencyPlan indegree
            bytes::<usize>(nodes)?, // later node ranks
            bytes::<usize>(canonical.return_tracks().len())?, // Return input indices
            bytes::<(usize, Owner)>(owners)?, // owner sort/input overlap
        ] {
            sum(&mut peak, size)?;
        }
        Ok(ConstructionCost {
            retained_bytes: retained,
            construction_peak_bytes: peak,
        })
    }
}

#[cfg(test)]
thread_local! { static OBSERVED_BEFORE_RETURN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "observe actual existing constructor locals without an owning test wrapper"
)]
pub(super) fn observe_before_zero_latency_return(
    builder: &Builder,
    tracks: &Vec<TrackPlan>,
    returns: &Vec<ReturnPlan>,
    inputs: &Vec<usize>,
    typed: &Vec<NodeId>,
    nodes: &Vec<LatencyNode>,
    edges: &Vec<LatencyEdge>,
    edge_keys: &Vec<u64>,
    plan: &LatencyPlan,
) {
    fn actual<T>(values: &Vec<T>) -> usize {
        values.capacity() * size_of::<T>()
    }
    let mut observed = actual(&builder.nodes)
        + actual(&builder.edges)
        + actual(tracks)
        + actual(returns)
        + actual(inputs)
        + actual(typed)
        + actual(nodes)
        + actual(edges)
        + actual(edge_keys)
        + plan.storage_bytes().unwrap();
    for track in tracks {
        observed += actual(&track.devices) + actual(&track.sends) + actual(&track.audio_feeds);
    }
    for bus in returns {
        observed += actual(&bus.devices) + actual(&bus.feeds) + actual(&bus.audio_feeds);
    }
    OBSERVED_BEFORE_RETURN.with(|value| value.set(value.get().max(observed)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created};
    #[test]
    fn graph_construction_cost_bounds_actual_retained_and_zero_latency_scratch() {
        assert!(crate::audio_storage_constructor_layout_supported());
        for delay in [0, 7] {
            for routed in [false, true] {
                let mut fixture = super::super::tests::fixture(&[&[delay], &[0]], 32);
                if routed {
                    let Created::ReturnTrack(first) = fixture
                        .editor
                        .execute(Command::AddReturnTrack {
                            name: "first".into(),
                        })
                        .unwrap()
                    else {
                        panic!("return")
                    };
                    let Created::ReturnTrack(second) = fixture
                        .editor
                        .execute(Command::AddReturnTrack {
                            name: "second".into(),
                        })
                        .unwrap()
                    else {
                        panic!("return")
                    };
                    fixture
                        .editor
                        .execute(Command::InsertReturnDevice {
                            return_track: first,
                            before: None,
                            state: super::super::tests::state(delay).unwrap(),
                        })
                        .unwrap();
                    fixture
                        .editor
                        .execute(Command::SetReturnSend {
                            track: fixture.tracks[0],
                            return_track: first,
                            gain: 0.5,
                        })
                        .unwrap();
                    let project = hex_project::Project::from_snapshot(
                        fixture
                            .editor
                            .project()
                            .snapshot()
                            .with_return_feed(first, second, 0.5)
                            .unwrap(),
                    )
                    .unwrap();
                    fixture.editor = hex_project::Editor::new(project, 8).unwrap();
                }
                crate::owned_shared::reset_birth_count();
                crate::owned_shared::reset_clone_count();
                let cost = SignalGraph::construction_cost(fixture.editor.project()).unwrap();
                assert_eq!(crate::owned_shared::birth_count(), 0);
                assert_eq!(crate::owned_shared::clone_count(), 0);
                OBSERVED_BEFORE_RETURN.with(|value| value.set(0));
                let revision = super::super::tests::prepare(&fixture).unwrap();
                let observed = OBSERVED_BEFORE_RETURN.with(|value| value.get());
                assert!(observed > 0);
                assert!(observed <= cost.construction_peak_bytes);
                let graph = revision.arrangement().signal_graph.as_ref();
                if delay == 0 {
                    assert!(graph.is_none());
                    assert!(
                        cost.construction_peak_bytes > 0,
                        "None does not make constructor scratch free"
                    );
                } else {
                    let graph = graph.unwrap();
                    let (_, header) = graph.parts().unwrap();
                    let retained = graph.table_bytes().unwrap() + header;
                    assert!(retained <= cost.retained_bytes);
                    assert!(retained <= cost.construction_peak_bytes);
                }
                eprintln!(
                    "SUPPORTED graph cost delay={delay} routed={routed} observed_partial_scratch={observed} retained_bound={} peak_bound={}",
                    cost.retained_bytes, cost.construction_peak_bytes
                );
            }
        }
    }
}
