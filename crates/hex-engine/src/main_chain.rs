//! Borrowed final-bus processor. Track/Return taps run before this one chain.
use crate::{Frame, NodeSignalObserver, PostNodeIdentity};
use hex_lattice::{NativeInstrumentClock, NativeRuntimeChain};

pub(crate) fn process(
    chain: &mut NativeRuntimeChain,
    frames: &mut [Frame],
    offset: usize,
    clock: Option<&dyn NativeInstrumentClock>,
    observer: &mut Option<&mut NodeSignalObserver<'_>>,
) -> Result<(), hex_lattice_contract::DeviceContractError> {
    if let Some(observer) = observer.as_mut() {
        chain.process_range_tapped_with_clock(
            0..chain.len(),
            frames,
            offset,
            clock,
            &mut |owner, device, kind, phase, offset, frames| {
                observer(
                    PostNodeIdentity::new(owner, device, kind),
                    phase,
                    offset,
                    frames,
                );
            },
        )
    } else {
        chain.process_range_with_clock(0..chain.len(), frames, clock)
    }
}
