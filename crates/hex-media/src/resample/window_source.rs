//! Borrowed mono/stereo access for the SAME indexed sinc kernel. No PCM owners.
use crate::ImportError;
use std::ops::Range;
pub(super) trait WindowSource {
    fn frame_count(&self) -> usize;
    fn sample(&self, index: usize) -> Option<[f32; 2]>;
    fn validate_finite(&self, range: Range<usize>) -> Result<(), ImportError>;
    fn copy_identity(&self, offset: usize, output: &mut [[f32; 2]]) -> Result<(), ImportError>;
}
fn missing() -> ImportError {
    ImportError::Invalid("identity source window is incomplete")
}
fn end(offset: usize, count: usize) -> Result<usize, ImportError> {
    offset
        .checked_add(count)
        .ok_or(ImportError::Invalid("identity source range overflow"))
}
impl WindowSource for [[f32; 2]] {
    fn frame_count(&self) -> usize {
        self.len()
    }
    fn sample(&self, index: usize) -> Option<[f32; 2]> {
        self.get(index).copied()
    }
    fn validate_finite(&self, range: Range<usize>) -> Result<(), ImportError> {
        let values = self.get(range).ok_or_else(missing)?;
        if values.iter().flatten().any(|v| !v.is_finite()) {
            Err(ImportError::Invalid("source samples must be finite"))
        } else {
            Ok(())
        }
    }
    fn copy_identity(&self, offset: usize, output: &mut [[f32; 2]]) -> Result<(), ImportError> {
        let values = self
            .get(offset..end(offset, output.len())?)
            .ok_or_else(missing)?;
        output.copy_from_slice(values);
        Ok(())
    }
}
impl WindowSource for [f32] {
    fn frame_count(&self) -> usize {
        self.len()
    }
    fn sample(&self, index: usize) -> Option<[f32; 2]> {
        self.get(index).map(|v| [*v; 2])
    }
    fn validate_finite(&self, range: Range<usize>) -> Result<(), ImportError> {
        let values = self.get(range).ok_or_else(missing)?;
        if values.iter().any(|v| !v.is_finite()) {
            Err(ImportError::Invalid("source samples must be finite"))
        } else {
            Ok(())
        }
    }
    fn copy_identity(&self, offset: usize, output: &mut [[f32; 2]]) -> Result<(), ImportError> {
        let values = self
            .get(offset..end(offset, output.len())?)
            .ok_or_else(missing)?;
        for (sample, frame) in values.iter().zip(output) {
            *frame = [*sample; 2];
        }
        Ok(())
    }
}
