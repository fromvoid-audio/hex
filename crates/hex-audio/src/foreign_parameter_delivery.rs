//! One exact VST3 operation observed at final direct Main output. The original
//! Root and PCM transport retain all native custody. This is a delivery witness,
//! separate from an SDK input ACK, canonical edit, recording take or loudspeaker.
//! One operation may be outstanding. Two one-cell SPSC rings are allocated at
//! output construction. Priority cancellation uses its own atomic marker.
use crate::{
    native_callback_owner::ChannelStorage,
    prepared::{AppliedAudioSource, Status},
};
use hex_lattice_contract::ForeignProcessorBinding as Binding;
use hex_plugin_host::transport::{AcceptedManualParameter, ManualParameterOrigin};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering::Acquire, Ordering::Release},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub source: AppliedAudioSource,
    pub binding: Binding,
    pub transport_epoch: u64,
    pub operation: u64,
    pub parameter: u32,
    serial: u64,
}
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
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Receipt {
    Delivered {
        target: Target,
        normalized: f64,
        origin: ManualParameterOrigin,
        /// The final direct Main sample's processing-clock position.
        delivery_performance_frame: u64,
    },
    Aborted {
        target: Target,
        reason: AbortReason,
    },
}
impl Receipt {
    fn target(self) -> Target {
        match self {
            Self::Delivered { target, .. } | Self::Aborted { target, .. } => target,
        }
    }
}
pub(crate) struct Control {
    commands: Producer<Target>,
    receipts: Consumer<Receipt>,
    cancellation: Arc<AtomicU64>,
    target: Option<Target>,
    serial: u64,
}
pub(crate) struct Callback {
    commands: Consumer<Target>,
    receipts: Producer<Receipt>,
    cancellation: Arc<AtomicU64>,
    target: Option<Target>,
    pending: Option<(u64, Receipt)>,
    terminal: Option<Receipt>,
}
pub(crate) fn channel() -> (Control, Callback) {
    let (commands, incoming) = RingBuffer::new(1);
    let (returning, receipts) = RingBuffer::new(1);
    let cancellation = Arc::new(AtomicU64::new(0));
    (
        Control {
            commands,
            receipts,
            cancellation: Arc::clone(&cancellation),
            target: None,
            serial: 0,
        },
        Callback {
            commands: incoming,
            receipts: returning,
            cancellation,
            target: None,
            pending: None,
            terminal: None,
        },
    )
}
pub(crate) fn constructor_storage_quote() -> Option<usize> {
    ChannelStorage::ring_bytes::<Target>(1)?
        .checked_add(ChannelStorage::ring_bytes::<Receipt>(1)?)?
        .checked_add(std::mem::size_of::<AtomicU64>())
}
impl Control {
    pub(crate) fn begin(
        &mut self,
        status: &Status,
        source: AppliedAudioSource,
        binding: Binding,
        operation: u64,
        parameter: u32,
    ) -> Result<Target, &'static str> {
        if self.target.is_some() || !self.receipts.is_empty() {
            return Err("drain the previous vst3 delivery witness first");
        }
        let (_, transport_epoch) = status
            .live_native_parameter_source()
            .filter(|(current, _)| *current == source)
            .ok_or("vst3 delivery source is not playing")?;
        if binding.root != source.output_identity || operation == 0 || parameter > i32::MAX as u32 {
            return Err("vst3 delivery target differs");
        }
        let serial = self
            .serial
            .checked_add(1)
            .ok_or("vst3 delivery serial exhausted")?;
        let target = Target {
            source,
            binding,
            transport_epoch,
            operation,
            parameter,
            serial,
        };
        status.reserve_native_parameter_input()?;
        if self.commands.push(target).is_err() {
            status.native_parameter_input_end();
            return Err("vst3 delivery witness is busy");
        }
        self.serial = serial;
        self.target = Some(target);
        Ok(target)
    }
    pub(crate) fn cancel(&mut self, target: Target) -> bool {
        if self.target != Some(target) {
            return false;
        }
        self.cancellation.store(target.serial, Release);
        true
    }
    pub(crate) fn try_receipt(&mut self, status: &Status) -> Option<Receipt> {
        let receipt = self.receipts.pop().ok()?;
        if self.target == Some(receipt.target()) {
            self.target = None;
            status.native_parameter_input_end();
        }
        Some(receipt)
    }
}
impl Callback {
    pub(crate) fn target(&self) -> Option<Target> {
        self.target
    }
    pub(crate) fn prepare(&mut self) {
        self.flush_terminal();
        if self.target.is_none()
            && self.terminal.is_none()
            && let Ok(target) = self.commands.pop()
        {
            self.target = Some(target);
        }
    }
    fn flush_terminal(&mut self) {
        if let Some(receipt) = self.terminal.take()
            && let Err(rtrb::PushError::Full(receipt)) = self.receipts.push(receipt)
        {
            self.terminal = Some(receipt);
        }
    }
    fn complete(&mut self, receipt: Receipt) {
        self.target = None;
        self.pending = None;
        self.terminal = Some(receipt);
        self.flush_terminal();
    }
    pub(crate) fn finish(
        &mut self,
        marker: Option<AcceptedManualParameter>,
        source: Option<(AppliedAudioSource, u64)>,
        end: u64,
        main_latency: u32,
        valid: bool,
        direct: bool,
    ) {
        let Some(target) = self.target else { return };
        let refused = if self.cancellation.load(Acquire) >= target.serial {
            Some(AbortReason::Cancelled)
        } else if !direct {
            Some(AbortReason::UnsupportedOutput)
        } else if !valid {
            Some(AbortReason::OutputFailed)
        } else if source.map(|(source, _)| source) != Some(target.source) {
            Some(AbortReason::SourceChanged)
        } else if source.map(|(_, epoch)| epoch) != Some(target.transport_epoch) {
            Some(AbortReason::TransportChanged)
        } else {
            None
        };
        if let Some(reason) = refused {
            self.complete(Receipt::Aborted { target, reason });
            return;
        }
        if let Some(marker) = marker
            && marker.binding == target.binding
            && marker.input.operation.get() >= target.operation
        {
            if marker.input.operation.get() != target.operation
                || marker.input.point.id != target.parameter
            {
                self.complete(Receipt::Aborted {
                    target,
                    reason: AbortReason::MissedOperation,
                });
                return;
            }
            let Some(origin) = marker.origin else {
                self.complete(Receipt::Aborted {
                    target,
                    reason: AbortReason::MissingOrigin,
                });
                return;
            };
            let Some(due) = origin
                .performance_frame
                .checked_add(u64::from(main_latency))
            else {
                self.complete(Receipt::Aborted {
                    target,
                    reason: AbortReason::ClockExhausted,
                });
                return;
            };
            self.pending = Some((
                due,
                Receipt::Delivered {
                    target,
                    normalized: marker.input.point.normalized,
                    origin,
                    delivery_performance_frame: due,
                },
            ));
        }
        if let Some((due, receipt)) = self.pending
            && due < end
        {
            self.complete(receipt);
        }
    }
    pub(crate) fn constructor_channel_storage(&self) -> Option<ChannelStorage> {
        ChannelStorage::ring(self.commands.buffer())?
            .plus(ChannelStorage::ring(self.receipts.buffer())?)?
            .plus(ChannelStorage {
                data_bytes: std::mem::size_of::<AtomicU64>(),
                shared_groups: 1,
                shared_handles: 2,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::AudioSourceContext;
    use hex_plugin_host::transport::{ManualParameterInput, ParameterPoint};
    use std::num::NonZeroU64;

    fn fixture() -> (Control, Callback, Target, AcceptedManualParameter) {
        let (mut control, mut callback) = channel();
        let source = AppliedAudioSource {
            context: AudioSourceContext {
                project_generation: 3,
                editor_revision: 7,
            },
            audio_revision: 9,
            output_identity: 11,
        };
        let binding = Binding {
            root: 11,
            instance: 13,
            graph: 17,
            epoch: 19,
        };
        let target = Target {
            source,
            binding,
            transport_epoch: 23,
            operation: 1,
            parameter: 0,
            serial: 1,
        };
        control.target = Some(target);
        control.commands.push(target).unwrap();
        callback.prepare();
        let marker = AcceptedManualParameter {
            binding,
            sequence: 5,
            first_frame: 40,
            callback_offset: 2,
            input: ManualParameterInput {
                operation: NonZeroU64::MIN,
                point: ParameterPoint {
                    id: 0,
                    frame: 0,
                    normalized: 0.12345678901234566,
                },
            },
            origin: Some(ManualParameterOrigin {
                arrangement_frame: 71,
                performance_frame: 100,
                loop_pass: 2,
            }),
        };
        (control, callback, target, marker)
    }

    #[test]
    fn exact_output_sample_is_exclusive_and_delivers_once_without_losing_precision() {
        let (mut control, mut callback, target, marker) = fixture();
        let source = Some((target.source, target.transport_epoch));
        callback.finish(Some(marker), source, 120, 30, true, true);
        assert!(control.receipts.is_empty());
        callback.finish(None, source, 130, 30, true, true);
        assert!(control.receipts.is_empty());
        callback.finish(None, source, 131, 30, true, true);
        assert_eq!(
            control.receipts.pop().unwrap(),
            Receipt::Delivered {
                target,
                normalized: marker.input.point.normalized,
                origin: marker.origin.unwrap(),
                delivery_performance_frame: 130,
            }
        );
        callback.finish(None, source, 132, 30, true, true);
        assert!(control.receipts.is_empty() && callback.target().is_none());
    }

    #[test]
    fn refusal_stale_clock_cancel_and_unknown_origin_cannot_acknowledge_delivery() {
        for expected in [
            AbortReason::Cancelled,
            AbortReason::SourceChanged,
            AbortReason::TransportChanged,
            AbortReason::OutputFailed,
            AbortReason::UnsupportedOutput,
            AbortReason::MissingOrigin,
            AbortReason::MissedOperation,
            AbortReason::ClockExhausted,
        ] {
            let (mut control, mut callback, target, mut marker) = fixture();
            let mut source = Some((target.source, target.transport_epoch));
            let mut valid = true;
            let mut direct = true;
            match expected {
                AbortReason::Cancelled => assert!(control.cancel(target)),
                AbortReason::SourceChanged => source = None,
                AbortReason::TransportChanged => source = Some((target.source, 24)),
                AbortReason::OutputFailed => valid = false,
                AbortReason::UnsupportedOutput => direct = false,
                AbortReason::MissingOrigin => marker.origin = None,
                AbortReason::MissedOperation => {
                    marker.input.operation = NonZeroU64::new(2).unwrap()
                }
                AbortReason::ClockExhausted => {
                    marker.origin.as_mut().unwrap().performance_frame = u64::MAX;
                }
            }
            callback.finish(Some(marker), source, 131, 30, valid, direct);
            assert_eq!(
                control.receipts.pop().unwrap(),
                Receipt::Aborted {
                    target,
                    reason: expected
                }
            );
        }
        let (mut control, mut callback, target, marker) = fixture();
        let source = Some((target.source, target.transport_epoch));
        callback.finish(Some(marker), source, 120, 30, true, true);
        assert!(control.cancel(target));
        callback.finish(None, source, 131, 30, true, true);
        assert_eq!(
            control.receipts.pop().unwrap(),
            Receipt::Aborted {
                target,
                reason: AbortReason::Cancelled
            }
        );
    }

    #[test]
    fn final_receipt_callback_neither_allocates_nor_frees_and_storage_is_bounded() {
        let (mut control, mut callback, target, marker) = fixture();
        let quote = constructor_storage_quote().unwrap();
        assert_eq!(
            callback.constructor_channel_storage().unwrap().data_bytes,
            quote
        );
        #[cfg(feature = "allocation-probe")]
        let measured = allocation_counter::measure(|| {
            callback.finish(
                Some(marker),
                Some((target.source, target.transport_epoch)),
                131,
                30,
                true,
                true,
            );
        });
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        #[cfg(not(feature = "allocation-probe"))]
        callback.finish(
            Some(marker),
            Some((target.source, target.transport_epoch)),
            131,
            30,
            true,
            true,
        );
        assert!(matches!(
            control.receipts.pop().unwrap(),
            Receipt::Delivered { .. }
        ));
    }
}
