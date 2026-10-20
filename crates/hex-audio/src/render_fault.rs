//! Fixed callback-to-control error snapshot. No formatting, allocation or I/O on publish.
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering::SeqCst};
const MESSAGE_BYTES: usize = 256;
const READ_ATTEMPTS: usize = 3;

/// Owned control-side diagnostic; never construct this on the audio callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderFaultSnapshot {
    pub message: String,
    pub audio_revision: u64,
    pub frame: u64,
    pub truncated: bool,
}

pub(crate) struct RenderFaultRecord {
    sequence: AtomicU64,
    length: AtomicU32,
    truncated: AtomicU8,
    revision: AtomicU64,
    frame: AtomicU64,
    message: [AtomicU8; MESSAGE_BYTES],
}
impl Default for RenderFaultRecord {
    fn default() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            length: AtomicU32::new(0),
            truncated: AtomicU8::new(0),
            revision: AtomicU64::new(0),
            frame: AtomicU64::new(0),
            message: std::array::from_fn(|_| AtomicU8::new(0)),
        }
    }
}
impl RenderFaultRecord {
    // Single callback writer, including clear after graph admission. SeqCst
    // prevents a reader accepting bytes from a different publication.
    pub(crate) fn publish(&self, message: &str, revision: u64, frame: u64) {
        let mut length = message.len().min(MESSAGE_BYTES);
        while !message.is_char_boundary(length) {
            length -= 1;
        }
        self.sequence.fetch_add(1, SeqCst);
        for (destination, byte) in self.message.iter().zip(message.as_bytes()).take(length) {
            destination.store(*byte, SeqCst);
        }
        self.length.store(length as u32, SeqCst);
        self.truncated
            .store(u8::from(length < message.len()), SeqCst);
        self.revision.store(revision, SeqCst);
        self.frame.store(frame, SeqCst);
        self.sequence.fetch_add(1, SeqCst);
    }
    pub(crate) fn clear(&self) {
        self.sequence.fetch_add(1, SeqCst);
        self.length.store(0, SeqCst);
        self.sequence.fetch_add(1, SeqCst);
    }
    pub(crate) fn read(&self) -> Option<RenderFaultSnapshot> {
        for _ in 0..READ_ATTEMPTS {
            let before = self.sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let length = usize::try_from(self.length.load(SeqCst)).ok()?;
            if length == 0 || length > MESSAGE_BYTES {
                return None;
            }
            let mut bytes = [0; MESSAGE_BYTES];
            for (destination, source) in bytes.iter_mut().zip(&self.message).take(length) {
                *destination = source.load(SeqCst);
            }
            let audio_revision = self.revision.load(SeqCst);
            let frame = self.frame.load(SeqCst);
            let truncated = self.truncated.load(SeqCst) != 0;
            if before != self.sequence.load(SeqCst) {
                continue;
            }
            return Some(RenderFaultSnapshot {
                message: std::str::from_utf8(&bytes[..length]).ok()?.to_owned(),
                audio_revision,
                frame,
                truncated,
            });
        }
        None
    }
}

impl RenderFaultRecord {
    pub(crate) fn audit_scalar_storage(&self) {
        let Self {
            sequence,
            length,
            truncated,
            revision,
            frame,
            message,
        } = self;
        let _: &AtomicU64 = sequence;
        let _: &AtomicU32 = length;
        let _: &AtomicU8 = truncated;
        let _: &AtomicU64 = revision;
        let _: &AtomicU64 = frame;
        let _: &[AtomicU8; MESSAGE_BYTES] = message;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_cause_context_utf8_truncation_and_recovery_are_preserved() {
        let record = RenderFaultRecord::default();
        assert!(record.read().is_none());
        record.publish("native instrument event batch invalid", 7, 1024);
        let first = record.read().expect("fault");
        assert_eq!(first.message, "native instrument event batch invalid");
        assert_eq!(
            (first.audio_revision, first.frame, first.truncated),
            (7, 1024, false)
        );
        let long = "é".repeat(200);
        record.publish(&long, 8, 2048);
        let truncated = record.read().expect("bounded fault");
        assert_eq!(truncated.message, "é".repeat(128));
        assert!(truncated.truncated);
        record.clear();
        assert!(record.read().is_none());
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn callback_fault_publication_and_clear_have_zero_total_and_net_allocations() {
        let record = RenderFaultRecord::default();
        let count = allocation_counter::measure(|| {
            record.publish("native device runtime unavailable", 9, 512);
            record.clear();
        });
        assert_eq!(
            (
                count.count_total,
                count.bytes_total,
                count.count_current,
                count.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
}
