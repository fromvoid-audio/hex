//! Selected actual exclusive model backings, not shared children/root admission.
use super::*;
use hex_project::{Project, SessionCellContent};
impl<'a> AudioRootStorageLedger<'a> {
    /// Actual container/String capacities, tempo/meter buffers and asset paths.
    /// Inline Project/Track/clip/Vec/String headers belong to enclosing storage.
    /// Shared MIDI/state/audio-edit/provenance children and root bodies, custody,
    /// construction peak/publication remain separate. Never call this complete.
    pub fn borrow_project_exclusive_buffers_on_control(
        &mut self,
        project: &'a Project,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| ledger.project_exclusive_buffers(project))
    }
    pub fn borrow_editor_exclusive_buffers_on_control(
        &mut self,
        editor: &'a hex_project::Editor,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| ledger.editor_exclusive_buffers(editor))
    }
    fn editor_exclusive_buffers(
        &mut self,
        editor: &'a hex_project::Editor,
    ) -> Result<(), AudioStorageError> {
        for project in editor.retained_projects_on_control() {
            self.project_exclusive_buffers(project)?;
        }
        Ok(())
    }
    fn model_string(&mut self, name: &'a String) -> Result<(), AudioStorageError> {
        self.bytes(
            Owner::exclusive(name),
            Location("model-string-buffer", 0),
            name.capacity(),
        )
    }
    fn project_exclusive_buffers(&mut self, project: &'a Project) -> Result<(), AudioStorageError> {
        let buffers = project.storage_buffers_on_control();
        self.vec(buffers.tracks())?;
        self.vec(buffers.returns())?;
        self.vec(buffers.routing())?;
        self.vec(buffers.audio_feeds())?;
        self.vec(buffers.midi_routes())?;
        self.vec(buffers.scenes())?;
        self.vec(buffers.cells())?;
        self.project_asset_storage(project)?;
        self.map(project.tempo_map())?;
        self.meter(project.meter_map())?;
        for track in buffers.tracks() {
            let buffers = track.storage_buffers_on_control();
            self.model_string(buffers.name())?;
            self.vec(buffers.devices())?;
            self.vec(buffers.audio_clips())?;
            self.vec(buffers.midi_clips())?;
            self.vec(buffers.sends())?;
            self.vec(buffers.gain_points())?;
            self.vec(buffers.gain_spans())?;
            for span in buffers.gain_spans() {
                self.vec(&span.steps)?;
            }
            for clip in buffers.audio_clips() {
                self.model_string(clip.name_buffer_on_control())?;
            }
            for clip in buffers.midi_clips() {
                self.model_string(clip.name_buffer_on_control())?;
            }
        }
        for track in buffers.returns() {
            self.model_string(track.name_buffer_on_control())?;
            self.vec(track.device_buffer_on_control())?;
        }
        for scene in buffers.scenes() {
            self.model_string(scene.name_buffer_on_control())?;
        }
        for cell in buffers.cells() {
            match cell.content() {
                SessionCellContent::Midi(clip) => {
                    self.model_string(clip.name_buffer_on_control())?
                }
                SessionCellContent::Audio(clip) => {
                    self.model_string(clip.name_buffer_on_control())?
                }
                SessionCellContent::Stop => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};
    fn reservation() -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: 256,
            handle_rows: 32,
            root_rows: 8,
            max_retained_bytes: 2_000_000,
            max_construction_peak_bytes: 2_000_000,
        }
    }
    fn bytes<T>(values: &Vec<T>) -> usize {
        values.capacity() * std::mem::size_of::<T>()
    }
    // This fixture contains a regular track, one scene, assets and timing maps;
    // no clip/gain/shared-child bytes are silently folded into the oracle.
    fn expected(project: &Project) -> usize {
        let b = project.storage_buffers_on_control();
        let mut result = bytes(b.tracks())
            + bytes(b.returns())
            + bytes(b.routing())
            + bytes(b.audio_feeds())
            + bytes(b.midi_routes())
            + bytes(b.scenes())
            + bytes(b.cells());
        result += project.asset_entries_on_control().backing_bytes().unwrap();
        result += project
            .assets()
            .map(|asset| asset.path_buffer_on_control().capacity())
            .sum::<usize>();
        result += project
            .tempo_map()
            .owned_allocation_layout()
            .checked_backing_bytes()
            .unwrap();
        result += project
            .meter_map()
            .owned_allocation_layout()
            .checked_backing_bytes()
            .unwrap();
        for track in b.tracks() {
            let b = track.storage_buffers_on_control();
            result += b.name().capacity()
                + bytes(b.devices())
                + bytes(b.audio_clips())
                + bytes(b.midi_clips())
                + bytes(b.sends())
                + bytes(b.gain_points())
                + bytes(b.gain_spans());
        }
        result += b
            .scenes()
            .iter()
            .map(|scene| scene.name_buffer_on_control().capacity())
            .sum::<usize>();
        result
    }
    #[test]
    fn actual_model_history_and_candidate_exclusive_buffers_count_independent_backings_once() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap();
        editor
            .execute(Command::AddAsset {
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 4,
            })
            .unwrap();
        editor
            .execute(Command::RenameTrack {
                track,
                name: "renamed".into(),
            })
            .unwrap();
        assert!(editor.undo());
        let candidate = editor.project().clone();
        let history: usize = editor.retained_projects_on_control().map(expected).sum();
        let full = history + expected(&candidate);
        assert!(full > history);
        let collect = |reservation| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
            ledger.borrow_editor_exclusive_buffers_on_control(&editor)?;
            assert_eq!(ledger.summary()?.data_bytes, history); // omitted candidate cannot match independent total
            ledger.borrow_project_exclusive_buffers_on_control(&candidate)?;
            assert_eq!(ledger.summary()?.data_bytes, full);
            ledger.borrow_editor_exclusive_buffers_on_control(&editor)?;
            ledger.borrow_project_asset_storage_on_control(&candidate)?;
            ledger
                .borrow_metadata_entries_backing_on_control(candidate.asset_entries_on_control())?;
            ledger.borrow_metadata_vec_backing_on_control(
                candidate.storage_buffers_on_control().tracks(),
            )?;
            assert_eq!(ledger.summary()?.data_bytes, full);
            assert!(ledger.summary()?.require_complete().is_err());
            ledger.summary()
        };
        let summary = collect(reservation()).unwrap();
        let mut exact = reservation();
        exact.max_retained_bytes = summary.retained_bytes;
        exact.max_construction_peak_bytes = summary.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), summary);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
    }
}
