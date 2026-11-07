//! Persistent teardown fields remain owned by the same lifecycle capsule.
use super::*;
impl SessionAssembly {
    pub(crate) fn has_next_disposal(&self) -> bool {
        !matches!(self.phase, AssemblyPhase::Vacant | AssemblyPhase::Complete)
    }
    fn unpack_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.inventory.is_none() {
            return Ok(());
        }
        // Every actual owner destination is empty BEFORE taking the aggregate.
        if self.scenes.capacity() != 0
            || self.tracks.capacity() != 0
            || self.cells.capacity() != 0
            || self.indices.capacity() != 0
            || self.ranges.capacity() != 0
            || self.audio.capacity() != 0
            || self.timing.is_some()
            || self.meter.is_some()
            || self.sine.is_some()
            || self.contexts.is_some()
            || self.unused_midi.capacity() != 0
            || self.unused_mutable.capacity() != 0
        {
            return Err(AudioStorageError::Capacity);
        }
        if self.inventory.as_ref().is_some_and(|inventory| {
            inventory.routed_midi_clips.capacity() != 0
                || inventory.midi_source_roots.capacity() != 0
        }) {
            return Err(AudioStorageError::UnknownStorage);
        }
        if let Some(PreparedSessionInventory {
            meter_map,
            routed_midi_clips: _,
            midi_source_roots: _,
            timing,
            periodic_tempo: _,
            periodic_tempo_error: _,
            source_admission_max_micro_bpm: _,
            sine_table,
            scenes,
            cells,
            scene_cell_indices,
            scene_cell_ranges,
            tracks,
            midi_clips,
            mutable_instruments,
            mutable_history_retarget_storage_bytes: _,
            audio_clips,
            work: _,
            contains_streamed_sources: _,
            midi_note_bank_storage_bytes: _,
            midi_note_bank_reference_count: _,
            shared_contexts,
        }) = self.inventory.take()
        {
            // Audio-only construction has zero-capacity unsupported vectors.
            // Keep them persistent too; never dispose an unexpected owner here.
            self.unused_midi = midi_clips;
            self.unused_mutable = mutable_instruments;
            self.timing = Some(timing);
            self.meter = Some(meter_map);
            self.sine = Some(sine_table);
            self.contexts = Some(shared_contexts);
            self.scenes = scenes;
            self.cells = cells;
            self.indices = scene_cell_indices;
            self.ranges = scene_cell_ranges;
            self.tracks = tracks;
            self.audio = audio_clips;
        }
        Ok(())
    }
    pub(crate) fn dispose_next(&mut self) -> Result<(), AudioStorageError> {
        self.unpack_on_control()?;
        self.phase = AssemblyPhase::Cleaning;
        if self.dispose_current_sampler_midi()? {
            return Ok(());
        }
        if !self.unused_mutable.is_empty() {
            self.current_sampler_bytes(None)?;
            // Only the fixed inline sampler factory constructed in this custody
            // may reach this control-only teardown, never a callback drop.
            drop(self.unused_mutable.pop());
            self.sampler_factories.pop();
            return Ok(());
        }
        if self.scene_cursor < self.scenes.len() {
            if self.name_pending {
                return Err(AudioStorageError::UnknownStorage);
            }
            self.name_pending = true;
            drop(mem::take(&mut self.scenes[self.scene_cursor].name));
            self.name_pending = false;
            self.scene_cursor += 1;
            return Ok(());
        }
        if self.track_cursor < self.tracks.len() {
            if self.name_pending {
                return Err(AudioStorageError::UnknownStorage);
            }
            self.name_pending = true;
            drop(mem::take(&mut self.tracks[self.track_cursor].name));
            self.name_pending = false;
            self.track_cursor += 1;
            return Ok(());
        }
        if let Some(index) = self.pending_pair {
            let row = &mut self.pairs[index];
            if row.source_disposal == Step::Unstarted {
                row.source_disposal = Step::Pending;
                drop(self.pending_source.take());
                #[cfg(test)]
                drop(self.disposal_probe.take());
                row.source_disposal = Step::Complete;
                return Ok(());
            }
            if row.source_disposal != Step::Complete {
                return Err(AudioStorageError::UnknownStorage);
            }
            if row.name_disposal == Step::Unstarted {
                row.name_disposal = Step::Pending;
                drop(self.pending_name.take());
                row.name_disposal = Step::Complete;
                self.pending_pair = None;
                return Ok(());
            }
            return Err(AudioStorageError::UnknownStorage);
        }
        let next = if !self.audio.is_empty() {
            Some(self.audio.len() - 1)
        } else {
            self.pairs.iter().rposition(|r| r.clip.is_some())
        };
        if let Some(index) = next {
            if self.pending_source.is_some()
                || self.pending_name.is_some()
                || self.pairs[index].source_disposal != Step::Unstarted
            {
                return Err(AudioStorageError::Capacity);
            }
            self.pending_pair = Some(index);
            let clip = if !self.audio.is_empty() {
                self.audio.pop()
            } else {
                self.pairs[index].clip.take()
            };
            if let Some(PreparedSessionAudioClip {
                active: _,
                id: _,
                track: _,
                scene: _,
                name,
                asset: _,
                source_start: _,
                source_end: _,
                loop_range: _,
                gain: _,
                source,
            }) = clip
            {
                self.pending_name = Some(name);
                self.pending_source = Some(source);
                return Ok(());
            }
            return Err(AudioStorageError::UnknownStorage);
        }
        if self.sine_disposal != Step::Complete {
            if self.sine_disposal != Step::Unstarted {
                return Err(AudioStorageError::UnknownStorage);
            }
            self.sine_disposal = Step::Pending;
            drop(self.sine.take());
            self.sine_disposal = Step::Complete;
            return Ok(());
        }
        if self.context_disposal != Step::Complete {
            if self.context_disposal != Step::Unstarted {
                return Err(AudioStorageError::UnknownStorage);
            }
            self.context_disposal = Step::Pending;
            drop(self.contexts.take());
            self.context_disposal = Step::Complete;
            return Ok(());
        }
        if self.meter.is_some() {
            self.meter_copy
                .clean_transferred_on_control(&mut self.meter)
                .map_err(meter_error)?;
        } else {
            self.meter_copy.clean_on_control().map_err(meter_error)?;
        }
        self.phase = AssemblyPhase::Complete;
        Ok(())
    }
    pub(crate) fn return_timing(
        &mut self,
        tempo: &mut SessionTempoConstruction,
    ) -> Result<(), AudioStorageError> {
        if self.timing.is_some() {
            tempo.restore_inventory_map(&mut self.timing)?;
        }
        Ok(())
    }
    pub(crate) fn clear_disposed(&mut self) -> Result<(), AudioStorageError> {
        if self.phase == AssemblyPhase::Vacant {
            return Ok(());
        }
        if self.phase != AssemblyPhase::Complete
            || self.inventory.is_some()
            || self.timing.is_some()
            || self.pending_pair.is_some()
            || self.pending_source.is_some()
            || self.pending_name.is_some()
            || self.sine.is_some()
            || self.contexts.is_some()
            || self.name_pending
            || self.scenes.iter().any(|s| s.name.capacity() != 0)
            || self.tracks.iter().any(|t| t.name.capacity() != 0)
            || self.pairs.iter().any(|p| {
                p.clip.is_some()
                    || (p.actual_handle.is_some()
                        && (p.source_disposal != Step::Complete
                            || p.name_disposal != Step::Complete))
            })
        {
            return Err(AudioStorageError::Capacity);
        }
        // Only empty headers/scalar declaration rows; backing remains charged.
        if !self.unused_midi.is_empty()
            || !self.sampler_midi.is_empty()
            || !self.sampler_factories.is_empty()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        drop(mem::take(&mut self.unused_midi));
        drop(mem::take(&mut self.sampler_midi));
        self.sampler_midi_scope = None;
        self.sampler_context_address = None;
        drop(mem::take(&mut self.unused_mutable));
        drop(mem::take(&mut self.sampler_factories));
        self.scenes.clear();
        self.tracks.clear();
        self.cells.clear();
        self.indices.clear();
        self.ranges.clear();
        self.pairs.clear();
        self.sine_declaration = None;
        self.metadata_handles = [0; 3];
        self.scene_cursor = 0;
        self.track_cursor = 0;
        self.sine_disposal = Step::Unstarted;
        self.context_disposal = Step::Unstarted;
        self.phase = AssemblyPhase::Vacant;
        Ok(())
    }
}
