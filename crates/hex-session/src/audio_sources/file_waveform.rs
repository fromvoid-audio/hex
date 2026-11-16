//! Peak-only ordinary file display preparation on the existing control worker.
//! Exact globally phased conversion uses bounded scratch; no source PCM, reader,
//! native endpoint or fabricated fine-resolution samples enter the display cache.
use super::*;
use hex_media::{
    ResampleKernel, ResamplePlan, ResampleVersion,
    waveform::{Peak, Waveform},
};
const MAX_BINS: usize = 1024;
const WINDOW: usize = 4096;
const MAX_SCRATCH: usize = 4096;
impl VerifiedAudioFile {
    /// Reuse display data only after the replacement file has been freshly
    /// inspected and every page has been verified again against that capture.
    pub(crate) fn reuse_waveform_on_control(
        &mut self,
        previous: &Self,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<bool, SessionError> {
        let Some(waveform) = previous
            .waveform
            .as_ref()
            .filter(|_| self.facts == previous.facts)
        else {
            return Ok(false);
        };
        let mut reader = self.open_waveform_reader(&mut cancelled)?;
        reader
            .track_complete_verification(self.reader_limits().max_manifest_bytes)?
            .finish(&mut cancelled)?;
        self.waveform = Some(Arc::clone(waveform));
        Ok(true)
    }
    pub(crate) fn cached_waveform(&self) -> Option<&Arc<Waveform>> {
        self.waveform.as_ref()
    }
    fn open_waveform_reader(
        &self,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<VerifiedWavReader, SessionError> {
        let reader = match VerifiedWavReader::open_verified_with_cancel(
            self.path(),
            self.reader_limits(),
            self.verified_manifest().clone(),
            &mut *cancelled,
        ) {
            Ok(reader) => reader,
            // An unchanged file may have a new mtime. Preserve the existing
            // complete fingerprint check before accepting it.
            Err(hex_media::ImportError::SourceChanged) => VerifiedWavReader::open_with_cancel(
                self.path(),
                self.reader_limits(),
                &mut *cancelled,
            )?,
            Err(error) => return Err(error.into()),
        };
        if reader.fingerprint() != self.fingerprint()
            || reader.sample_rate() != self.source_rate()
            || reader.source_frames() != self.source_frames()
        {
            return Err(SessionError::Invalid(
                "waveform file differs from captured source",
            ));
        }
        Ok(reader)
    }
    pub(crate) fn prepare_waveform_on_control(
        &mut self,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<(), SessionError> {
        if self.waveform.is_some() {
            return Ok(());
        }
        let mut reader = self.open_waveform_reader(&mut cancelled)?;
        let frames = usize::try_from(self.playback_frames())
            .map_err(|_| SessionError::Invalid("file waveform extent exceeds platform"))?;
        let count = frames.min(MAX_BINS);
        if count == 0 {
            return Err(SessionError::Invalid("empty file waveform"));
        }
        let kernel = ResampleKernel::with_version(
            self.source_rate(),
            self.project_rate(),
            ResampleVersion::for_conversion(self.conversion_version())?,
        )?;
        let plan = ResamplePlan::with_kernel(
            kernel,
            self.source_frames(),
            self.source_frames(),
            self.playback_frames(),
            WINDOW,
        )?;
        let scratch_count = reader.max_window_frames().min(MAX_SCRATCH);
        let mut coverage =
            reader.track_complete_verification(self.reader_limits().max_manifest_bytes)?;
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(scratch_count)
            .map_err(|_| SessionError::Allocation)?;
        scratch.resize(scratch_count, [0.0; 2]);
        let mut output = [[0.0; 2]; WINDOW];
        let mut bins = Vec::new();
        bins.try_reserve_exact(count)
            .map_err(|_| SessionError::Allocation)?;
        let width = frames / count;
        let extra = frames % count;
        let mut cursor = 0usize;
        let mut index = 0usize;
        let mut end = width + usize::from(extra != 0);
        let mut peaks: Option<[Peak; 2]> = None;
        let mut identical_channels = true;
        // Decode contiguous bounded windows across peak-bin boundaries. Opening
        // a fresh verified read for each tiny bin repeatedly hashes the same raw
        // header/data pages; bin aggregation does not require that extra I/O.
        while cursor < frames {
            if cancelled() {
                return Err(SessionError::Invalid("file waveform preparation cancelled"));
            }
            let mut take = (frames - cursor).min(WINDOW);
            loop {
                let (first, last) = plan.required_source_range(cursor as u64, take)?;
                if last
                    .checked_sub(first)
                    .is_some_and(|count| count <= scratch_count as u64)
                {
                    break;
                }
                if take == 1 {
                    return Err(SessionError::Invalid(
                        "file waveform source window exceeds reservation",
                    ));
                }
                take /= 2;
            }
            plan.render_verified_coverage_window(
                &mut coverage,
                cursor as u64,
                &mut output[..take],
                &mut scratch,
            )?;
            let mut offset = 0;
            while offset < take {
                let len = (end - cursor).min(take - offset);
                let segment = &output[offset..offset + len];
                let rest = if peaks.is_none() {
                    let Some((first, rest)) = segment.split_first() else {
                        return Err(SessionError::Invalid("empty file waveform bin"));
                    };
                    identical_channels &= first[0].to_bits() == first[1].to_bits();
                    peaks = Some(first.map(|value| Peak {
                        min: value,
                        max: value,
                    }));
                    rest
                } else {
                    segment
                };
                if let Some(peaks) = peaks.as_mut() {
                    for frame in rest {
                        identical_channels &= frame[0].to_bits() == frame[1].to_bits();
                        for (peak, value) in peaks.iter_mut().zip(frame) {
                            peak.min = peak.min.min(*value);
                            peak.max = peak.max.max(*value);
                        }
                    }
                }
                cursor += len;
                offset += len;
                if cursor == end {
                    bins.push(
                        peaks
                            .take()
                            .ok_or(SessionError::Invalid("empty file waveform bin"))?,
                    );
                    index += 1;
                    if index < count {
                        end = end
                            .checked_add(width)
                            .and_then(|n| n.checked_add(usize::from(index < extra)))
                            .ok_or(SessionError::Invalid("file waveform range overflow"))?;
                    }
                }
            }
        }
        if cursor != frames || index != count || peaks.is_some() || cancelled() {
            return Err(SessionError::Invalid(
                "file waveform preparation cancelled or incomplete",
            ));
        }
        coverage.finish(&mut cancelled)?;
        let waveform = Waveform::from_peaks(frames, bins)?;
        let waveform = if identical_channels {
            waveform.with_identical_channels()?
        } else {
            waveform
        };
        self.waveform = Some(Arc::new(waveform));
        Ok(())
    }
}
