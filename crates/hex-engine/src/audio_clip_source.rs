//! Private restriction of the existing source ownership family for clip commit.
//! Resource-only layouts must refuse before a source owner enters this type.
use crate::{
    Audio, AudioStorageError, StreamedSourceMetadata, audio_only_inputs::SourceKind,
    owned_shared::OwnedShared,
};
pub(crate) enum ClipSourceKind {
    Resident(Audio),
    Streamed(OwnedShared<StreamedSourceMetadata>),
}
pub(crate) struct OwnedAudioClipSource {
    pub(crate) kind: ClipSourceKind,
    pub(crate) handle: u64,
}
impl OwnedAudioClipSource {
    pub(crate) fn frames(&self) -> u64 {
        match &self.kind {
            ClipSourceKind::Resident(audio) => audio.frames().len() as u64,
            ClipSourceKind::Streamed(source) => source.playback_frames(),
        }
    }
}
impl ClipSourceKind {
    pub(crate) fn try_clone_source_on_control(
        source: &SourceKind,
        handle: u64,
    ) -> Result<Self, AudioStorageError> {
        Ok(match source {
            SourceKind::Resident(audio) => {
                Self::Resident(audio.clone_with_native_handle_on_control(Some(handle)))
            }
            SourceKind::ResidentMono(_) => return Err(AudioStorageError::UnsupportedGraph),
            SourceKind::Streamed(metadata) => Self::Streamed(metadata.clone_on_control()),
        })
    }
    pub(crate) fn clone_on_control(&self, handle: u64) -> Self {
        match self {
            Self::Resident(audio) => {
                Self::Resident(audio.clone_with_native_handle_on_control(Some(handle)))
            }
            Self::Streamed(metadata) => Self::Streamed(metadata.clone_on_control()),
        }
    }
}
impl crate::audio_only_inputs::SourceStorage for OwnedAudioClipSource {
    fn storage_kind(&self) -> crate::audio_only_inputs::SourceStorageKind<'_> {
        use crate::audio_only_inputs::SourceStorageKind;
        match &self.kind {
            ClipSourceKind::Resident(audio) => SourceStorageKind::Resident(audio),
            ClipSourceKind::Streamed(source) => SourceStorageKind::Streamed(source),
        }
    }
    fn storage_handle(&self) -> u64 {
        self.handle
    }
}
