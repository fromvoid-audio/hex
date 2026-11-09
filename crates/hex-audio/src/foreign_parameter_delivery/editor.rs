//! Output-side custody for passive exact editor observations. A separate
//! terminal cell is reserved; a full reader never silently drops the prefix.
use super::AbortReason;
use crate::{
    native_callback_owner::ChannelStorage,
    prepared::{AppliedAudioSource, Status},
};
use hex_lattice_contract::ForeignProcessorBinding as Binding;
use hex_plugin_host::transport::{AcceptedEditorQuantum, EditorScope};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{
            AtomicU64,
            Ordering::{Acquire, Release},
        },
    },
};
mod frontier;
use frontier::Frontier;
pub use frontier::{ConfirmedQuantum, Target};

const RECEIPTS: usize = 65;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Receipt {
    Confirmed(ConfirmedQuantum),
    Aborted { target: Target, reason: AbortReason },
}

#[derive(Default)]
struct Signals {
    cancelled: AtomicU64,
    frontier_scope: AtomicU64,
    // Sequence + 1, since sequence zero is a legitimate native PCM quantum.
    frontier: AtomicU64,
}

pub(crate) struct Control {
    commands: Producer<Target>,
    receipts: Consumer<Receipt>,
    signals: Arc<Signals>,
    target: Option<(Target, EditorScope)>,
    serial: u64,
}

pub(crate) struct Callback {
    commands: Consumer<Target>,
    receipts: Producer<Receipt>,
    signals: Arc<Signals>,
    target: Option<Target>,
    frontier: Frontier,
}

pub(crate) fn channel() -> (Control, Callback) {
    let (commands, incoming) = RingBuffer::new(1);
    let (returning, receipts) = RingBuffer::new(RECEIPTS);
    let signals = Arc::new(Signals::default());
    (
        Control {
            commands,
            receipts,
            signals: Arc::clone(&signals),
            target: None,
            serial: 0,
        },
        Callback {
            commands: incoming,
            receipts: returning,
            signals,
            target: None,
            frontier: Frontier::new(),
        },
    )
}

pub(crate) fn constructor_storage_quote() -> Option<usize> {
    ChannelStorage::ring_bytes::<Target>(1)?
        .checked_add(ChannelStorage::ring_bytes::<Receipt>(RECEIPTS)?)?
        .checked_add(std::mem::size_of::<Signals>())?
        .checked_add(Frontier::storage_quote()?)
}

impl Control {
    pub(crate) fn begin(
        &mut self,
        status: &Status,
        source: AppliedAudioSource,
        binding: Binding,
        arm: impl FnOnce(NonZeroU64) -> Result<(EditorScope, u16), &'static str>,
    ) -> Result<Target, &'static str> {
        if self.target.is_some() || !self.receipts.is_empty() || self.commands.slots() == 0 {
            return Err("drain the previous vst3 editor delivery first");
        }
        let (_, transport_epoch) = status
            .live_native_parameter_source()
            .filter(|(current, _)| *current == source)
            .ok_or("vst3 editor delivery source is not playing")?;
        if !binding.is_valid() || binding.root != source.output_identity {
            return Err("vst3 editor delivery source differs");
        }
        let scope = self
            .serial
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or("vst3 editor delivery scope exhausted")?;
        // Consume this serial even if the exact owner is already closing.
        // The original worker will never accept a reused serial.
        self.serial = scope.get();
        let (armed, quantum_frames) = arm(scope)?;
        let target = Target {
            source,
            binding,
            transport_epoch,
            scope,
            quantum_frames,
        };
        self.commands
            .push(target)
            .map_err(|_| "vst3 editor delivery is busy")?;
        self.target = Some((target, armed));
        Ok(target)
    }

    pub(crate) fn cancel(&self, target: Target) -> bool {
        if self.target.as_ref().map(|(current, _)| *current) != Some(target) {
            return false;
        }
        self.signals.cancelled.store(target.scope.get(), Release);
        true
    }

    pub(crate) fn try_receipt(&mut self) -> Option<Receipt> {
        let receipt = self.receipts.pop().ok()?;
        if let Receipt::Aborted { target, .. } = receipt
            && self.target.as_ref().map(|(current, _)| *current) == Some(target)
        {
            // Only control disarms or drops the exact original worker signal.
            self.target = None;
        }
        Some(receipt)
    }

    /// Latest native quantum whose first sample passed the final host format.
    /// Values for that prefix were enqueued before this publication. Readers
    /// first capture this frontier, then drain the corresponding PCM receipts.
    pub(crate) fn confirmed_sequence(&self, target: Target) -> Option<u64> {
        let scope = target.scope.get();
        if self.target.as_ref().map(|(current, _)| *current) != Some(target)
            || self.signals.frontier_scope.load(Acquire) != scope
        {
            return None;
        }
        let sequence = self.signals.frontier.load(Acquire).checked_sub(1)?;
        (self.signals.frontier_scope.load(Acquire) == scope).then_some(sequence)
    }
}

impl Callback {
    pub(crate) fn prepare_target(&mut self) -> Option<Target> {
        if self.target.is_none()
            && let Ok(target) = self.commands.pop()
        {
            self.signals.frontier_scope.store(0, Release);
            self.signals.frontier.store(0, Release);
            self.signals
                .frontier_scope
                .store(target.scope.get(), Release);
            self.target = Some(target);
        }
        self.target
    }

    pub(crate) fn finish(
        &mut self,
        marker: Option<AcceptedEditorQuantum>,
        source: Option<(AppliedAudioSource, u64)>,
        end: u64,
        main_latency: u32,
        valid: bool,
        direct: bool,
    ) {
        let Some(target) = self.target else { return };
        let refused = if self.signals.cancelled.load(Acquire) >= target.scope.get() {
            Some(AbortReason::Cancelled)
        } else if !direct {
            Some(AbortReason::UnsupportedOutput)
        } else if !valid {
            Some(AbortReason::OutputFailed)
        } else if source.map(|(current, _)| current) != Some(target.source) {
            Some(AbortReason::SourceChanged)
        } else if source.map(|(_, epoch)| epoch) != Some(target.transport_epoch) {
            Some(AbortReason::TransportChanged)
        } else {
            None
        };
        let confirmation = if let Some(reason) = refused {
            Err(reason)
        } else {
            self.frontier.finish(target, marker, end, main_latency)
        };
        let failure = match confirmation {
            Ok(None) => return,
            Ok(Some(quantum)) => {
                let Some(prefix) = quantum.input.sequence.checked_add(1) else {
                    self.abort(target, AbortReason::ClockExhausted);
                    return;
                };
                if !quantum.input.performs.as_slice().is_empty()
                    && (self.receipts.slots() <= 1
                        || self.receipts.push(Receipt::Confirmed(quantum)).is_err())
                {
                    AbortReason::Saturated
                } else {
                    self.signals.frontier.store(prefix, Release);
                    return;
                }
            }
            Err(reason) => reason,
        };
        self.abort(target, failure);
    }

    fn abort(&mut self, target: Target, reason: AbortReason) {
        self.target = None;
        self.frontier.clear();
        // Exactly one terminal per command. Confirmed batches never use
        // the last slot, and a new command requires the terminal's receipt.
        let _ = self.receipts.push(Receipt::Aborted { target, reason });
    }

    pub(crate) fn constructor_channel_storage(&self) -> Option<ChannelStorage> {
        ChannelStorage::ring(self.commands.buffer())?
            .plus(ChannelStorage::ring(self.receipts.buffer())?)?
            .plus(ChannelStorage {
                data_bytes: std::mem::size_of::<Signals>()
                    .checked_add(self.frontier.heap_bytes()?)?,
                shared_groups: 1,
                shared_handles: 2,
            })
    }
}
