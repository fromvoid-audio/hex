//! Shared bounded non-blocking pipe/socket draining helpers for scanner supervisors.
use std::io::{Error, ErrorKind, Read};

pub const DRAIN_CHUNK_BYTES: usize = 4 * 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrainOutcome {
    pub any_read: bool,
    pub eof: bool,
}

pub fn final_drain_passes(frame_limit: usize) -> usize {
    frame_limit
        .max(64 * 1024)
        .div_ceil(DRAIN_CHUNK_BYTES)
        .saturating_add(2)
}

/// Drain up to 4 chunks (16 KiB) from `reader` into `bytes` without exceeding `limit`.
pub fn drain_bounded(
    reader: &mut impl Read,
    bytes: &mut Vec<u8>,
    limit: usize,
    overflow_error: &str,
    read_error: impl FnOnce(Error) -> String,
) -> Result<DrainOutcome, String> {
    let mut buffer = [0_u8; 4096];
    let mut any_read = false;
    for _ in 0..4 {
        match reader.read(&mut buffer) {
            Ok(0) => {
                return Ok(DrainOutcome {
                    any_read,
                    eof: true,
                });
            }
            Ok(count) => {
                any_read = true;
                if bytes.len().checked_add(count).is_none_or(|n| n > limit) {
                    return Err(overflow_error.into());
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                return Ok(DrainOutcome {
                    any_read,
                    eof: false,
                });
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(read_error(error)),
        }
    }
    Ok(DrainOutcome {
        any_read,
        eof: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct ScriptedReader(VecDeque<std::io::Result<Vec<u8>>>);

    impl Read for ScriptedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop_front() {
                None => Ok(0),
                Some(Ok(chunk)) => {
                    let count = chunk.len().min(buf.len());
                    buf[..count].copy_from_slice(&chunk[..count]);
                    Ok(count)
                }
                Some(Err(err)) => Err(err),
            }
        }
    }

    #[test]
    fn final_drain_passes_covers_arbitrary_frame_limit() {
        let large_limit = 4 * 1024 * 1024 + crate::framing::FRAME_HEADER;
        assert!(final_drain_passes(large_limit) * DRAIN_CHUNK_BYTES >= large_limit);
    }

    #[test]
    fn drain_bounded_reports_reads_eof_would_block_and_bounds() {
        let mut reader = ScriptedReader(VecDeque::from([
            Err(Error::new(ErrorKind::Interrupted, "retry")),
            Ok(b"hello".to_vec()),
            Err(Error::new(ErrorKind::WouldBlock, "later")),
            Ok(b" world".to_vec()),
            Ok(Vec::new()),
        ]));
        let mut out = Vec::new();
        let first =
            drain_bounded(&mut reader, &mut out, 32, "overflow", |e| e.to_string()).unwrap();
        assert_eq!(
            first,
            DrainOutcome {
                any_read: true,
                eof: false
            }
        );
        assert_eq!(out, b"hello");

        let second =
            drain_bounded(&mut reader, &mut out, 32, "overflow", |e| e.to_string()).unwrap();
        assert_eq!(
            second,
            DrainOutcome {
                any_read: true,
                eof: true
            }
        );
        assert_eq!(out, b"hello world");

        let mut oversized = ScriptedReader(VecDeque::from([Ok(vec![1_u8; 5])]));
        let err = drain_bounded(&mut oversized, &mut Vec::new(), 4, "overflow", |e| {
            e.to_string()
        })
        .unwrap_err();
        assert_eq!(err, "overflow");
    }
}
