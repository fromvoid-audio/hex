//! Actual constructor-paired topology, separate from mutable delay/runtime
//! buffers and from whole graph construction/publication qualification.
use super::*;
use crate::{AudioRootStorageLedger, AudioStorageError, audio_storage::Owner as StorageOwner};

pub(crate) fn visit<'a>(
    graph: &'a OwnedShared<SignalGraph>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    if ledger.skip_observed_shared_field(graph) {
        return Ok(());
    }
    let visited = ledger.skip_observed_shared_contents(graph.arc());
    ledger.observe_shared(graph.arc(), graph.parts(), StorageOwner::exclusive(graph))?;
    if visited {
        return Ok(());
    }
    let SignalGraph {
        plan,
        tracks,
        returns,
        owner_order,
        main,
        main_devices,
        pre_main_edges,
        typed,
    } = graph.as_ref();
    fn scalar<T: Copy>(_: &T) {}
    scalar(main);
    fn elements<T: Copy>(_: &Vec<T>) {}
    elements(owner_order);
    elements(typed);
    plan.visit_storage_on_control(ledger)?;
    ledger.vec(tracks)?;
    ledger.vec(returns)?;
    ledger.vec(owner_order)?;
    ledger.vec(typed)?;
    ledger.vec(main_devices)?;
    ledger.vec(pre_main_edges)?;
    for track in tracks {
        let TrackPlan {
            id,
            devices,
            generator,
            merge,
            fader,
            source_input,
            audio_merge,
            generator_merge,
            main,
            sends,
            audio_feeds,
        } = track;
        scalar(&(
            *id,
            *generator,
            *merge,
            *fader,
            *source_input,
            *audio_merge,
            *generator_merge,
            *main,
        ));
        elements(devices);
        ledger.vec(devices)?;
        ledger.vec(sends)?;
        ledger.vec(audio_feeds)?;
        for send in sends.iter().chain(audio_feeds) {
            leaf(send);
        }
    }
    for bus in returns {
        let ReturnPlan {
            id,
            devices,
            output,
            main,
            feeds,
            audio_feeds,
        } = bus;
        scalar(&(*id, *output, *main));
        elements(devices);
        ledger.vec(devices)?;
        ledger.vec(feeds)?;
        ledger.vec(audio_feeds)?;
        for send in feeds.iter().chain(audio_feeds) {
            leaf(send);
        }
    }
    Ok(())
}
fn leaf(send: &Send) {
    let Send { target, gain, edge } = send;
    fn scalar<T: Copy>(_: T) {}
    scalar((*target, *gain, *edge));
}

/// Actual new runtime buffers and its independent physical graph alias. The
/// enclosing Session runtime charges the inline SignalRuntime body already.
pub(crate) fn visit_runtime<'a>(
    runtime: &'a SignalRuntime,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let SignalRuntime {
        graph,
        delay_graph,
        delays,
        inputs,
        main,
    } = runtime;
    visit(graph, ledger)?;
    visit(delay_graph, ledger)?;
    delays.visit_storage_on_control(ledger)?;
    ledger.vec(inputs)?;
    ledger.vec(main)
}
