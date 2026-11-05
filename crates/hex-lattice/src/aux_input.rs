//! Validated borrowed stereo auxiliary input; the host retains all buffer custody.
use crate::StereoFrame;
use std::fmt;

/// Why a host-provided block cannot be delivered to a native aux processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuxiliaryBlockError {
    InvalidMaximum,
    BlockTooLarge,
    LengthMismatch,
    SilenceBufferTooSmall,
}

impl fmt::Display for AuxiliaryBlockError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::InvalidMaximum => "auxiliary block maximum must be nonzero",
            Self::BlockTooLarge => "auxiliary block exceeds the prepared maximum",
            Self::LengthMismatch => "auxiliary input length differs from the main block",
            Self::SilenceBufferTooSmall => "auxiliary silence buffer is too small",
        })
    }
}
impl std::error::Error for AuxiliaryBlockError {}

/// One paired main/key block. The key is always the same length as the main
/// block, including when disconnected. All slices remain host-owned and may
/// not be retained after processing. `maximum_frames` is the bound admitted
/// by the host for this runtime, not a value inferred from the incoming data.
pub struct StereoAuxiliaryBlock<'a> {
    main: &'a mut [StereoFrame],
    key: &'a [StereoFrame],
}

impl<'a> StereoAuxiliaryBlock<'a> {
    /// Validate before processing. If `key` is absent or unavailable, zero the
    /// borrowed scratch prefix and use it as the key; the main input is never
    /// substituted. No allocation, locking, or retained ownership occurs.
    pub fn new(
        main: &'a mut [StereoFrame],
        key: Option<&'a [StereoFrame]>,
        silence: &'a mut [StereoFrame],
        maximum_frames: usize,
    ) -> Result<Self, AuxiliaryBlockError> {
        if maximum_frames == 0 {
            return Err(AuxiliaryBlockError::InvalidMaximum);
        }
        if main.len() > maximum_frames {
            return Err(AuxiliaryBlockError::BlockTooLarge);
        }
        let length = main.len();
        let key = match key {
            Some(key) if key.len() == length => key,
            Some(_) => return Err(AuxiliaryBlockError::LengthMismatch),
            None => {
                let Some(scratch) = silence.get_mut(..length) else {
                    return Err(AuxiliaryBlockError::SilenceBufferTooSmall);
                };
                scratch.fill([0.0; 2]);
                scratch
            }
        };
        Ok(Self { main, key })
    }

    /// Reborrow a matching subrange of an already validated block.
    pub(crate) fn paired(main: &'a mut [StereoFrame], key: &'a [StereoFrame]) -> Self {
        Self { main, key }
    }

    /// Split the validated block into its mutable main and immutable key views.
    pub fn into_parts(self) -> (&'a mut [StereoFrame], &'a [StereoFrame]) {
        (self.main, self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EMPTY_DEVICE_RESOURCES, NativeProcessorRuntime, conformance::auxiliary::AuxiliaryProbe,
    };

    #[test]
    fn connected_and_disconnected_blocks_cover_every_admitted_size() {
        const MAX: usize = 256;
        let mut runtime = AuxiliaryProbe::default();
        for length in 1..=MAX {
            let mut main = [[2.0, -3.0]; MAX];
            let key = [[0.5, -0.5]; MAX];
            let mut scratch = [[7.0; 2]; MAX];
            let block = StereoAuxiliaryBlock::new(
                &mut main[..length],
                Some(&key[..length]),
                &mut scratch,
                MAX,
            )
            .unwrap();
            runtime
                .process_block_with_auxiliary(block, None, None, false, &EMPTY_DEVICE_RESOURCES)
                .unwrap();
            assert!(main[..length].iter().all(|frame| *frame == [1.0, 1.5]));
            assert!(scratch.iter().all(|frame| *frame == [7.0; 2]));

            main.fill([2.0, -3.0]);
            let block =
                StereoAuxiliaryBlock::new(&mut main[..length], None, &mut scratch, MAX).unwrap();
            runtime
                .process_block_with_auxiliary(block, None, None, false, &EMPTY_DEVICE_RESOURCES)
                .unwrap();
            assert!(main[..length].iter().all(|frame| *frame == [0.0; 2]));
            assert!(scratch[..length].iter().all(|frame| *frame == [0.0; 2]));
            assert!(scratch[length..].iter().all(|frame| *frame == [7.0; 2]));
        }
    }

    #[test]
    fn invalid_lengths_refuse_before_any_buffer_changes_and_bypass_stays_dry() {
        let mut main = [[2.0, -3.0]; 4];
        let key = [[0.5, -0.5]; 3];
        let mut scratch = [[7.0; 2]; 4];
        assert!(matches!(
            StereoAuxiliaryBlock::new(&mut main, Some(&key), &mut scratch, 4),
            Err(AuxiliaryBlockError::LengthMismatch)
        ));
        assert!(matches!(
            StereoAuxiliaryBlock::new(&mut main, None, &mut scratch[..3], 4),
            Err(AuxiliaryBlockError::SilenceBufferTooSmall)
        ));
        assert!(matches!(
            StereoAuxiliaryBlock::new(&mut main, None, &mut scratch, 3),
            Err(AuxiliaryBlockError::BlockTooLarge)
        ));
        assert!(matches!(
            StereoAuxiliaryBlock::new(&mut main, None, &mut scratch, 0),
            Err(AuxiliaryBlockError::InvalidMaximum)
        ));
        assert_eq!(main, [[2.0, -3.0]; 4]);
        assert_eq!(scratch, [[7.0; 2]; 4]);
        let mut runtime = AuxiliaryProbe::default();
        let key = [[0.5, -0.5]; 4];
        let block = StereoAuxiliaryBlock::new(&mut main, Some(&key), &mut scratch, 4).unwrap();
        runtime
            .process_block_with_auxiliary(block, None, None, true, &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        assert_eq!(main, [[2.0, -3.0]; 4]);
        assert_eq!(runtime.frames_seen, 4);
    }
}
