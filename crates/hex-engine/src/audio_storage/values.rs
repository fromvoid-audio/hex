use super::*;
impl<'a> AudioRootStorageLedger<'a> {
    pub(crate) fn clip(
        &mut self,
        clip: &'a crate::Clip,
        declaration: Option<&crate::owned_shared::SharedDeclaration>,
        handle: u64,
    ) -> Result<(), AudioStorageError> {
        let crate::Clip {
            active: _,
            audio,
            start: _,
            end: _,
            looping: _,
            source_offset: _,
            source_length: _,
            metadata,
        } = clip;
        let streamed = metadata.as_ref().and_then(|m| m.streamed.as_ref());
        if audio.is_some() == streamed.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        if let Some(metadata) = metadata {
            self.clip_metadata(metadata)?;
        }
        if let Some(audio) = audio {
            self.resident(audio, Owner::exclusive(audio), handle)?;
        }
        if let Some(streamed) = streamed {
            self.shared(
                streamed,
                declaration.and_then(|d| d.parts(streamed)),
                Owner::exclusive(streamed),
                handle,
                Some(streamed.as_ref()),
            )?;
        }
        Ok(())
    }
    pub(crate) fn clip_metadata(
        &mut self,
        metadata: &'a crate::ClipMetadata,
    ) -> Result<(), AudioStorageError> {
        let crate::ClipMetadata {
            gain: _,
            source_phase: _,
            reversed: _,
            fade: _,
            streamed: _,
            sparse,
        } = metadata;
        self.bytes(
            Owner::exclusive(metadata),
            Location("clip-metadata-box", 0),
            std::mem::size_of::<crate::ClipMetadata>(),
        )?;
        if let Some(map) = sparse {
            self.sparse_map(map)?;
        }
        Ok(())
    }
    pub(crate) fn sparse_map(
        &mut self,
        map: &'a std::sync::Arc<crate::SparseFrameMap>,
    ) -> Result<(), AudioStorageError> {
        self.bytes(
            Owner::shared(map),
            Location("sparse-frame-map", 0),
            map.retained_bytes()
                .ok_or(AudioStorageError::UnknownStorage)?,
        )
    }
    pub(crate) fn map(&mut self, map: &'a hex_project::TempoMap) -> Result<(), AudioStorageError> {
        let layout = map.owned_allocation_layout();
        self.bytes(
            Owner::exclusive(map),
            Location("tempo-points", 0),
            layout.point_bytes().ok_or(AudioStorageError::Overflow)?,
        )?;
        self.bytes(
            Owner::exclusive(map),
            Location("tempo-prefix", 0),
            layout.prefix_bytes().ok_or(AudioStorageError::Overflow)?,
        )
    }
    pub(crate) fn meter(
        &mut self,
        map: &'a hex_project::MeterMap,
    ) -> Result<(), AudioStorageError> {
        let layout = map.owned_allocation_layout();
        self.bytes(
            Owner::exclusive(map),
            Location("meter-points", 0),
            layout.point_bytes().ok_or(AudioStorageError::Overflow)?,
        )?;
        self.bytes(
            Owner::exclusive(map),
            Location("meter-prefix", 0),
            layout.prefix_bytes().ok_or(AudioStorageError::Overflow)?,
        )
    }
}
