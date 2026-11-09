//! Output-side custody for passive exact editor observations. A separate
//! terminal cell is reserved; a full reader never silently drops the prefix.
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

/// Confirmed Main values and the one independently reserved terminal.
pub const MAX_EDITOR_MAIN_RECEIPTS: usize = 65;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortReason {
    Cancelled,
    SourceChanged,
    TransportChanged,
    OutputFailed,
    UnsupportedOutput,
    MissingOrigin,
    ClockExhausted,
    MissedOperation,
    Saturated,
}

#[allow(clippy::large_enum_variant)] // Fixed inline callback receipts; boxing would allocate.
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

/// The exact selected parameter and its original fallback, established by
/// the source owner before this active scope may enter the SDK.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchPlan {
    pub parameter: u32,
    pub base: f64,
    pub reading: bool,
}

struct Reservation(Arc<Status>);
impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.native_parameter_input_end();
    }
}

struct Armed {
    target: Target,
    _scope: EditorScope,
    _reservation: Option<Reservation>,
}

pub(crate) struct Control {
    commands: Producer<Target>,
    receipts: Consumer<Receipt>,
    signals: Arc<Signals>,
    target: Option<Armed>,
    serial: u64,
}

/// The original editor receipt queue after its publication control relinquishes
/// the output. It retains signals and the bounded receipt prefix, never a
/// Root, device, native worker, callback carrier or output lease.
pub struct Retired {
    control: Control,
    target: Target,
}

impl Retired {
    pub fn target(&self) -> Target {
        self.target
    }

    pub fn cancel_on_control(&self) -> bool {
        self.control.cancel(self.target)
    }

    pub fn try_receipt_on_control(&mut self) -> Option<Receipt> {
        self.control.try_receipt()
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        if let Some(target) = self.target() {
            self.cancel(target);
        }
    }
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
    let (returning, receipts) = RingBuffer::new(MAX_EDITOR_MAIN_RECEIPTS);
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
        .checked_add(ChannelStorage::ring_bytes::<Receipt>(
            MAX_EDITOR_MAIN_RECEIPTS,
        )?)?
        .checked_add(std::mem::size_of::<Signals>())?
        .checked_add(Frontier::storage_quote()?)
}

impl Control {
    pub(crate) fn target(&self) -> Option<Target> {
        self.target.as_ref().map(|armed| armed.target)
    }

    pub(crate) fn retire(self) -> Option<Retired> {
        let target = self.target()?;
        Some(Retired {
            control: self,
            target,
        })
    }

    pub(crate) fn begin(
        &mut self,
        status: &Status,
        source: AppliedAudioSource,
        binding: Binding,
        expected_epoch: Option<u64>,
        exclusive: Option<Arc<Status>>,
        arm: impl FnOnce(NonZeroU64) -> Result<(EditorScope, u16), &'static str>,
    ) -> Result<Target, &'static str> {
        if self.target.is_some() || !self.receipts.is_empty() || self.commands.slots() == 0 {
            return Err("drain the previous vst3 editor delivery first");
        }
        let (_, transport_epoch) = status
            .live_native_parameter_source()
            .filter(|(current, epoch)| {
                *current == source && expected_epoch.is_none_or(|expected| expected == *epoch)
            })
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
        let reservation = exclusive
            .map(|status| {
                status.reserve_native_parameter_input()?;
                Ok::<_, &'static str>(Reservation(status))
            })
            .transpose()?;
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
        self.target = Some(Armed {
            target,
            _scope: armed,
            _reservation: reservation,
        });
        Ok(target)
    }

    pub(crate) fn cancel(&self, target: Target) -> bool {
        if self.target.as_ref().map(|armed| armed.target) != Some(target) {
            return false;
        }
        self.signals.cancelled.store(target.scope.get(), Release);
        true
    }

    pub(crate) fn try_receipt(&mut self) -> Option<Receipt> {
        let receipt = match self.receipts.pop() {
            Ok(receipt) => receipt,
            Err(_) if self.receipts.is_abandoned() => {
                // Preserve anything enqueued before callback return. An
                // absent terminal never confirms a pending physical reply.
                std::sync::atomic::fence(Acquire);
                self.receipts.pop().ok().or_else(|| {
                    let target = self.target.as_ref()?.target;
                    Some(Receipt::Aborted {
                        target,
                        reason: AbortReason::OutputFailed,
                    })
                })?
            }
            Err(_) => return None,
        };
        if let Receipt::Aborted { target, .. } = receipt
            && self.target.as_ref().map(|armed| armed.target) == Some(target)
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
        if self.target.as_ref().map(|armed| armed.target) != Some(target)
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
                if (!quantum.input.performs.as_slice().is_empty()
                    || quantum.input.restoration.is_some())
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
                // Two signal aliases and, for an exclusive editor touch,
                // one optional alias of the already charged output status.
                shared_handles: 3,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_plugin_host::transport::ManualParameterOrigin;

    fn target() -> Target {
        Target {
            source: AppliedAudioSource {
                context: hex_engine::AudioSourceContext {
                    project_generation: 2,
                    editor_revision: 3,
                },
                audio_revision: 5,
                output_identity: 7,
            },
            binding: Binding {
                root: 7,
                instance: 11,
                graph: 13,
                epoch: 17,
            },
            transport_epoch: 19,
            scope: NonZeroU64::MIN,
            quantum_frames: 8,
        }
    }

    #[test]
    fn a_pending_physical_reply_cannot_survive_a_transport_or_output_change() {
        let target = target();
        for reason in [
            AbortReason::SourceChanged,
            AbortReason::TransportChanged,
            AbortReason::OutputFailed,
        ] {
            let (mut control, mut callback) = channel();
            control.commands.push(target).unwrap();
            assert_eq!(callback.prepare_target(), Some(target));
            callback.finish(
                Some(AcceptedEditorQuantum {
                    binding: target.binding,
                    sequence: 1,
                    first_frame: 8,
                    callback_offset: 2,
                    scope: target.scope,
                    performs: Default::default(),
                    restoration: None,
                    lost_origin: false,
                    origin: Some(ManualParameterOrigin {
                        arrangement_frame: 2,
                        performance_frame: 8,
                        loop_pass: 1,
                    }),
                }),
                Some((target.source, target.transport_epoch)),
                26,
                20,
                true,
                true,
            );
            assert!(control.try_receipt().is_none());
            let source = match reason {
                AbortReason::SourceChanged => None,
                AbortReason::TransportChanged => Some((target.source, target.transport_epoch + 1)),
                _ => Some((target.source, target.transport_epoch)),
            };
            callback.finish(
                None,
                source,
                30,
                20,
                reason != AbortReason::OutputFailed,
                true,
            );
            assert_eq!(
                control.try_receipt(),
                Some(Receipt::Aborted { target, reason })
            );
            assert_eq!(callback.prepare_target(), None);
            assert_eq!(control.signals.frontier.load(Acquire), 0);
            assert!(control.try_receipt().is_none());
        }
    }
}
