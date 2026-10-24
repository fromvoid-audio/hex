//! Control-side PCM-buffer capacity accounting; never graph lifetime authority.
use crate::{PreparedImport, SessionError};
use hex_engine::Audio;

impl PreparedImport {
    pub(crate) fn retained_pcm_frames(&self) -> Result<u64, SessionError> {
        self.additional_pcm_frames(std::iter::empty())
    }

    pub(crate) fn additional_pcm_frames<'a>(
        &self,
        existing: impl Iterator<Item = &'a Audio> + Clone,
    ) -> Result<u64, SessionError> {
        let mut frames = 0_u64;
        for (index, audio) in [&self.source_audio, &self.audio].into_iter().enumerate() {
            if (index == 1 && audio.shares_pcm_with(&self.source_audio))
                || existing.clone().any(|known| audio.shares_pcm_with(known))
            {
                continue;
            }
            frames = frames
                .checked_add(u64::try_from(audio.pcm_capacity_frames()).map_err(|_| {
                    SessionError::Invalid("resident pcm capacity exceeds frame range")
                })?)
                .ok_or(SessionError::Invalid("resident pcm capacity overflow"))?;
        }
        Ok(frames)
    }
}

#[cfg(test)]
mod tests;
