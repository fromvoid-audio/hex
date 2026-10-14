use super::*;
use crate::{Clip, ClipMetadata, PreparedRevision, Project, RendererRuntime, Track};
impl<'a, V: PlaybackStorageVisitor<'a>> Report<'a, '_, V> {
    pub(crate) fn project(&mut self, project: &'a Arc<Project>) -> Result<(), StorageReportError> {
        let owner = self.arc(project, "arrangement")?;
        let Project {
            sample_rate: _,
            tracks,
            return_buses,
            return_feed_plan: _,
            loop_region: _,
            master_gain: _,
            audible_end_frame: _,
            has_streamed_sources: _,
            shared_contexts,
        } = project.as_ref();
        self.vec(owner, "arrangement.tracks", [0; 2], tracks)?;
        self.vec(owner, "arrangement.returns", [0; 2], return_buses)?;
        self.unknown(
            owner,
            "arrangement.return_feed_plan",
            UnknownStorageReason::NestedOwnerContract,
        )?;
        self.opaque_arc(shared_contexts, "shared_context_table")?;
        for (index, track) in tracks.iter().enumerate() {
            self.track(track, owner, index)?;
        }
        for bus in return_buses {
            self.opaque_arc(&bus.device_chain, "processor_instance_slice")?;
        }
        Ok(())
    }
    fn track(
        &mut self,
        track: &'a Track,
        owner: StorageOwner<'a>,
        index: usize,
    ) -> Result<(), StorageReportError> {
        let Track {
            track_id: _,
            clips,
            mix: _,
            source_mix: _,
            gain_envelope,
            instrument,
            instrument_position: _,
            ordered_prefix: _,
            ordered_suffix: _,
            device_chain,
            sends,
            instrument_id: _,
            instrument_state,
            instrument_bypassed: _,
            instrument_output_gain: _,
            instrument_meter,
            mutable_instrument: _,
            mutable_looping_source: _,
        } = track;
        self.vec(owner, "track.clips", [index, 0], clips)?;
        self.vec(owner, "track.sends", [index, 0], sends)?;
        if gain_envelope.is_some() {
            self.unknown(
                owner,
                "track.gain_envelope",
                UnknownStorageReason::NestedOwnerContract,
            )?;
        }
        if instrument.is_some() {
            self.unknown(
                owner,
                "track.instrument",
                UnknownStorageReason::NestedOwnerContract,
            )?;
        }
        if instrument_state.is_some() {
            self.unknown(
                owner,
                "track.instrument_state",
                UnknownStorageReason::NestedOwnerContract,
            )?;
        }
        self.opaque_arc(device_chain, "processor_instance_slice")?;
        self.opaque_arc(instrument_meter, "device_meter")?;
        for (clip_index, clip) in clips.iter().enumerate() {
            self.clip(clip, owner, [index, clip_index])?;
        }
        Ok(())
    }
    fn clip(
        &mut self,
        clip: &'a Clip,
        owner: StorageOwner<'a>,
        indices: [usize; 2],
    ) -> Result<(), StorageReportError> {
        let Clip {
            audio,
            start: _,
            end: _,
            looping: _,
            source_offset: _,
            source_length: _,
            metadata,
        } = clip;
        if let Some(audio) = audio {
            let crate::Audio {
                sample_rate: _,
                frames,
            } = audio;
            self.arc(frames, "resident_pcm")?;
        }
        if let Some(metadata) = metadata {
            self.data(
                owner,
                "clip.metadata_box",
                indices,
                1,
                std::mem::size_of::<ClipMetadata>(),
            )?;
            let ClipMetadata {
                source_phase: _,
                fade: _,
                streamed,
            } = metadata.as_ref();
            if let Some(streamed) = streamed {
                self.opaque_arc(streamed, "streamed_source_metadata")?;
            }
        }
        Ok(())
    }
    pub(crate) fn revision(
        &mut self,
        revision: &'a Arc<PreparedRevision>,
    ) -> Result<(), StorageReportError> {
        let owner = self.arc(revision, "prepared_revision")?;
        // Private nested metadata is not assumed to have zero storage.
        self.unknown(
            owner,
            "revision.nested_owners",
            UnknownStorageReason::NestedOwnerContract,
        )?;
        self.project(revision.arrangement())?;
        self.opaque_arc(revision.session(), "session_inventory")?;
        self.opaque_arc(revision.midi_preview(), "midi_preview")?;
        Ok(())
    }
    pub(crate) fn opaque_arc<T: 'static + ?Sized>(
        &mut self,
        value: &'a Arc<T>,
        category: &'static str,
    ) -> Result<(), StorageReportError> {
        let owner = self.arc(value, category)?;
        self.unknown(owner, category, UnknownStorageReason::NestedOwnerContract)
    }
    pub(crate) fn chain(
        &mut self,
        chain: &hex_lattice::NativeRuntimeChain,
    ) -> Result<(), StorageReportError> {
        let bytes = chain
            .declared_runtime_bytes()
            .checked_mul(2)
            .ok_or(StorageReportError::ArithmeticOverflow)?;
        self.item(
            StorageOwner::Exclusive,
            "chain.dsp_twins",
            [0; 2],
            StorageCharge::ProviderDeclared { bytes },
        )?;
        // Default Unknown: entries capacity and opaque/configuration/meter owners
        // require the owning lattice module's audited report. No guessed bytes.
        self.unknown(
            StorageOwner::Exclusive,
            "chain.owners",
            UnknownStorageReason::OpaqueProvider,
        )
    }
    pub(crate) fn renderer(&mut self, runtime: &RendererRuntime) -> Result<(), StorageReportError> {
        let RendererRuntime {
            device_chains,
            return_chains,
            return_scratch,
            runtime_error: _,
        } = runtime;
        self.vec(
            StorageOwner::Exclusive,
            "renderer.device_chains",
            [0; 2],
            device_chains,
        )?;
        self.vec(
            StorageOwner::Exclusive,
            "renderer.return_chains",
            [0; 2],
            return_chains,
        )?;
        self.vec(
            StorageOwner::Exclusive,
            "renderer.return_scratch",
            [0; 2],
            return_scratch,
        )?;
        for chain in device_chains.iter().chain(return_chains) {
            self.chain(chain)?;
        }
        Ok(())
    }
}
