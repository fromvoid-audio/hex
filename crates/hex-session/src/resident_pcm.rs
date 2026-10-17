//! Control-side PCM-buffer capacity accounting; never graph lifetime authority.
use crate::ResidentPcm;
use crate::{PreparedImport, SessionError};

impl PreparedImport {
    pub(crate) fn retained_pcm_frames(&self) -> Result<u64, SessionError> {
        self.additional_pcm_frames(std::iter::empty())
    }

    pub(crate) fn additional_pcm_frames<'a>(
        &self,
        existing: impl Iterator<Item = ResidentPcm<'a>> + Clone,
    ) -> Result<u64, SessionError> {
        let mut frames = 0_u64;
        for (index, audio) in self.pcm().into_iter().enumerate() {
            if (index == 1 && audio.shares(self.source_audio.pcm()))
                || existing.clone().any(|known| audio.shares(known))
            {
                continue;
            }
            frames = frames
                .checked_add(
                    u64::try_from(
                        audio
                            .capacity_frames()
                            .ok_or(SessionError::Invalid("nonresident original pcm"))?,
                    )
                    .map_err(|_| {
                        SessionError::Invalid("resident pcm capacity exceeds frame range")
                    })?,
                )
                .ok_or(SessionError::Invalid("resident pcm capacity overflow"))?;
        }
        Ok(frames)
    }
}

#[cfg(test)]
mod tests;
