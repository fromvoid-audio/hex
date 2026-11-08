//! Once-only scalar receipt transfer after matched stream disposal and both callback guard
//! releases. The detached reader retains only Copy records, never an output/resource lease.
use super::*;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarReceiptTarget {
    Gain(crate::gain_recording::Target),
    NativeParameter(crate::native_parameter_input::Target),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScalarReceipt {
    Gain(crate::gain_recording::Receipt),
    NativeParameter(crate::native_parameter_input::Receipt),
}
impl ScalarReceipt {
    fn target(self) -> ScalarReceiptTarget {
        match self {
            Self::Gain(r) => ScalarReceiptTarget::Gain(r.target()),
            Self::NativeParameter(r) => ScalarReceiptTarget::NativeParameter(r.target()),
        }
    }
    fn terminal(self) -> Option<ScalarReceiptTerminal> {
        use crate::{gain_recording::Receipt as G, native_parameter_input::Receipt as N};
        match self {
            Self::Gain(G::Finished { .. }) | Self::NativeParameter(N::Finished { .. }) => {
                Some(ScalarReceiptTerminal::Finished)
            }
            Self::Gain(G::Aborted { .. }) | Self::NativeParameter(N::Aborted { .. }) => {
                Some(ScalarReceiptTerminal::Aborted)
            }
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarReceiptTerminal {
    Finished,
    Aborted,
    /// Quiescence is established, but no actual matching terminal was delivered.
    Interrupted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetiredReceiptFailure {
    Unavailable,
}
#[derive(Clone, Copy, Debug)]
pub struct RetiredScalarReceiptBatch {
    target: ScalarReceiptTarget,
    slots: [Option<ScalarReceipt>; crate::live_delivery::CAPACITY],
    len: usize,
    terminal: ScalarReceiptTerminal,
}
impl RetiredScalarReceiptBatch {
    pub fn target(&self) -> ScalarReceiptTarget {
        self.target
    }
    pub fn receipts(&self) -> impl Iterator<Item = ScalarReceipt> + '_ {
        self.slots[..self.len].iter().flatten().copied()
    }
    pub fn terminal(&self) -> ScalarReceiptTerminal {
        self.terminal
    }
}
pub(super) type Mailbox = OnceLock<Result<RetiredScalarReceiptBatch, RetiredReceiptFailure>>;
/// Non-Clone, bounded once-only observation. Pending does not imply exhaustion or failure.
pub struct RetiredScalarReceipts {
    mailbox: Arc<Mailbox>,
    output_identity: u64,
    target: ScalarReceiptTarget,
    taken: bool,
}
impl RetiredScalarReceipts {
    pub fn output_identity(&self) -> u64 {
        self.output_identity
    }
    pub fn target(&self) -> ScalarReceiptTarget {
        self.target
    }
    pub fn try_take(&mut self) -> Option<Result<RetiredScalarReceiptBatch, RetiredReceiptFailure>> {
        if self.taken {
            return None;
        }
        let result = *self.mailbox.get()?;
        self.taken = true;
        Some(result)
    }
}
pub(crate) struct ReceiptPublisher(Arc<Mailbox>);
impl ReceiptPublisher {
    pub(crate) fn identity(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
    pub(crate) fn fail(&self) {
        let _ = self.0.set(Err(RetiredReceiptFailure::Unavailable));
    }
    pub(crate) fn collect(&self, endpoints: &mut ClientEndpoints) {
        let Some(target) = endpoints.retained_receipt_target else {
            return;
        };
        let mut batch = RetiredScalarReceiptBatch {
            target,
            slots: [None; crate::live_delivery::CAPACITY],
            len: 0,
            terminal: ScalarReceiptTerminal::Interrupted,
        };
        loop {
            let receipt = match target {
                ScalarReceiptTarget::Gain(_) => endpoints
                    .gain_recording
                    .try_receipt()
                    .map(ScalarReceipt::Gain),
                ScalarReceiptTarget::NativeParameter(_) => endpoints
                    .native_parameter_input
                    .try_receipt()
                    .map(ScalarReceipt::NativeParameter),
            };
            let Some(receipt) = receipt else { break };
            if receipt.target() != target || batch.len == batch.slots.len() {
                self.fail();
                return;
            }
            if let Some(terminal) = receipt.terminal() {
                batch.terminal = terminal;
            }
            batch.slots[batch.len] = Some(receipt);
            batch.len += 1;
        }
        let _ = self.0.set(Ok(batch));
    }
}
pub(super) fn channel() -> (Arc<Mailbox>, ReceiptPublisher) {
    let mailbox = Arc::new(OnceLock::new());
    (Arc::clone(&mailbox), ReceiptPublisher(mailbox))
}
pub(crate) fn storage_bytes() -> usize {
    std::mem::size_of::<Mailbox>()
}
impl Client {
    pub fn close_retaining_gain_receipts(&mut self) -> Option<RetiredScalarReceipts> {
        let target = self
            .endpoints
            .owner_ref()
            .and_then(|e| e.gain_recording.target())
            .map(ScalarReceiptTarget::Gain);
        self.close_retaining_scalar_receipts(target)
    }
    pub fn close_retaining_native_parameter_receipts(&mut self) -> Option<RetiredScalarReceipts> {
        let target = self
            .endpoints
            .owner_ref()
            .and_then(|e| e.native_parameter_input.target())
            .map(ScalarReceiptTarget::NativeParameter);
        self.close_retaining_scalar_receipts(target)
    }
    fn close_retaining_scalar_receipts(
        &mut self,
        target: Option<ScalarReceiptTarget>,
    ) -> Option<RetiredScalarReceipts> {
        let token = target.and_then(|target| {
            let endpoints = self.endpoints.owner_mut()?;
            endpoints.retained_receipt_target = Some(target);
            Some(RetiredScalarReceipts {
                mailbox: self.retired_receipts.take()?,
                output_identity: endpoints.output_identity,
                target,
                taken: false,
            })
        });
        self.close();
        token
    }
    pub(crate) fn receipt_mailbox_identity(&self) -> Option<usize> {
        self.retired_receipts
            .as_ref()
            .map(|m| Arc::as_ptr(m) as usize)
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests;
