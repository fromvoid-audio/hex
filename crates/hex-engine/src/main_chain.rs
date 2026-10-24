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
    process_with_foreign(
        chain,
        frames,
        offset,
        clock,
        &mut hex_lattice::RefuseForeign,
        observer,
    )
}

pub(crate) fn process_with_foreign(
    chain: &mut NativeRuntimeChain,
    frames: &mut [Frame],
    offset: usize,
    clock: Option<&dyn NativeInstrumentClock>,
    foreign: &mut dyn hex_lattice::ForeignProcessors,
    observer: &mut Option<&mut NodeSignalObserver<'_>>,
) -> Result<(), hex_lattice_contract::DeviceContractError> {
    if let Some(observer) = observer.as_mut() {
        chain.process_range_tapped_with_foreign(
            0..chain.len(),
            frames,
            offset,
            clock,
            foreign,
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
        chain.process_range_with_foreign(0..chain.len(), frames, offset, clock, foreign)
    }
}

/// Control-side final arrival. The graph's Main node still aligns only its
/// incoming Track/Return paths; serial Main processing adds no upstream delay.
pub(crate) fn output_latency(
    pre_main: u32,
    devices: &[hex_lattice::PreparedRuntimeInstance],
) -> Result<u32, crate::Invalid> {
    let limit = crate::compensation::CompensationLimits::default().max_cumulative_frames;
    let mut arrival = pre_main;
    if arrival > limit {
        return Err(crate::Invalid(
            "cumulative latency exceeds the compensation frame limit",
        ));
    }
    for device in devices {
        let latency = device
            .algorithmic_latency_frames()
            .ok_or(crate::Invalid("enabled processor latency is unknown"))?;
        arrival = arrival
            .checked_add(latency)
            .filter(|frames| *frames <= limit)
            .ok_or(crate::Invalid(
                "cumulative latency exceeds the compensation frame limit",
            ))?;
    }
    Ok(arrival)
}
