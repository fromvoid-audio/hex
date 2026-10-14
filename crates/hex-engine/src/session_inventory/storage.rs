use super::*;
use crate::storage_report::{PlaybackStorageVisitor, Report, StorageOwner, StorageReportError};
impl PreparedSessionInventory {
    /// All other inventory owners remain explicitly Unknown in the caller.
    pub(crate) fn visit_source_templates_and_tempo<'a, V: PlaybackStorageVisitor<'a>>(
        &'a self,
        report: &mut Report<'a, '_, V>,
        owner: StorageOwner<'a>,
    ) -> Result<(), StorageReportError> {
        let Self {
            timing,
            periodic_tempo: _,
            periodic_tempo_error: _,
            source_admission_max_micro_bpm: _,
            sine_table: _,
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
        } = self;
        report.vec(owner, "inventory.scenes", [0; 2], scenes)?;
        report.vec(owner, "inventory.cells", [0; 2], cells)?;
        report.vec(
            owner,
            "inventory.scene_cell_indices",
            [0; 2],
            scene_cell_indices,
        )?;
        report.vec(
            owner,
            "inventory.scene_cell_ranges",
            [0; 2],
            scene_cell_ranges,
        )?;
        report.vec(owner, "inventory.tracks", [0; 2], tracks)?;
        report.vec(owner, "inventory.midi_clips", [0; 2], midi_clips)?;
        report.vec(owner, "inventory.audio_clips", [0; 2], audio_clips)?;
        report.vec(
            owner,
            "inventory.mutable_instruments",
            [0; 2],
            mutable_instruments,
        )?;
        for (index, scene) in scenes.iter().enumerate() {
            let PreparedSessionScene { id: _, name } = scene;
            report.data(
                owner,
                "inventory.scene_name",
                [index, 0],
                name.capacity(),
                1,
            )?;
        }
        for (index, track) in tracks.iter().enumerate() {
            let PreparedSessionTrack {
                id: _,
                name,
                gain: _,
                balance: _,
                muted: _,
                soloed: _,
                work: _,
            } = track;
            report.data(
                owner,
                "inventory.track_name",
                [index, 0],
                name.capacity(),
                1,
            )?;
        }
        for (index, clip) in midi_clips.iter().enumerate() {
            let PreparedSessionMidiClip {
                id: _,
                track: _,
                scene: _,
                name,
                source_identity: _,
                source: _,
                instrument: _,
                instrument_id: _,
                instrument_state: _,
                instrument_enabled: _,
                mutable_limits: _,
                work: _,
            } = clip;
            report.data(owner, "inventory.midi_name", [index, 0], name.capacity(), 1)?;
        }
        for (index, clip) in audio_clips.iter().enumerate() {
            let PreparedSessionAudioClip {
                id: _,
                track: _,
                scene: _,
                name,
                asset: _,
                source_start: _,
                source_end: _,
                source: _,
            } = clip;
            report.data(
                owner,
                "inventory.audio_name",
                [index, 0],
                name.capacity(),
                1,
            )?;
        }
        report.context_table(shared_contexts, "session_inventory_contexts")?;
        timing.visit_storage(report, owner)?;
        for clip in &self.midi_clips {
            let owner = report.owned_arc(&clip.source, "session_midi_source_template")?;
            clip.source.visit_storage(report, owner)?;
            let owner = report.owned_arc(&clip.instrument, "session_midi_instrument_template")?;
            clip.instrument.visit_owned_storage(report, owner)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_report::{StorageOwner, owned_tests::Sink};
    use hex_project::{Command, Created, Editor};
    #[test]
    fn owned_inventory_capacities_strings_and_actual_owner_dedup() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "clip".into(),
                source_length: hex_project::BeatTime::from_quarters(1).unwrap(),
            })
            .unwrap();
        let Created::Scene(audio_scene) = editor
            .execute(Command::AddScene {
                name: "audio scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "owned.wav".into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene: audio_scene,
                name: "audio".into(),
                asset,
                source_start: 0,
                source_end: 8,
            })
            .unwrap();
        let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.0; 2]; 8]).unwrap())]);
        let mut inventory = PreparedSessionInventory::prepare_with_media(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap();
        inventory.scenes.reserve(11);
        inventory.cells.reserve(12);
        inventory.scene_cell_indices.reserve(13);
        inventory.scene_cell_ranges.reserve(14);
        inventory.tracks.reserve(15);
        inventory.midi_clips.reserve(16);
        inventory.audio_clips.reserve(17); // Empty with real spare capacity.
        inventory.mutable_instruments.reserve(18);
        inventory.scenes[0].name.reserve(25);
        inventory.tracks[0].name.reserve(26);
        inventory.midi_clips[0].name.reserve(27);
        inventory.audio_clips[0].name.reserve(28);
        let expected = [
            (
                "inventory.scenes",
                inventory.scenes.capacity(),
                std::mem::size_of::<PreparedSessionScene>(),
            ),
            (
                "inventory.cells",
                inventory.cells.capacity(),
                std::mem::size_of::<PreparedSessionCell>(),
            ),
            (
                "inventory.scene_cell_indices",
                inventory.scene_cell_indices.capacity(),
                std::mem::size_of::<usize>(),
            ),
            (
                "inventory.scene_cell_ranges",
                inventory.scene_cell_ranges.capacity(),
                std::mem::size_of::<PreparedSessionSceneCellRange>(),
            ),
            (
                "inventory.tracks",
                inventory.tracks.capacity(),
                std::mem::size_of::<PreparedSessionTrack>(),
            ),
            (
                "inventory.midi_clips",
                inventory.midi_clips.capacity(),
                std::mem::size_of::<PreparedSessionMidiClip>(),
            ),
            (
                "inventory.audio_clips",
                inventory.audio_clips.capacity(),
                std::mem::size_of::<PreparedSessionAudioClip>(),
            ),
            (
                "inventory.mutable_instruments",
                inventory.mutable_instruments.capacity(),
                std::mem::size_of::<PreparedMutableTrackInstrument>(),
            ),
            (
                "inventory.scene_name",
                inventory.scenes[0].name.capacity(),
                1,
            ),
            (
                "inventory.track_name",
                inventory.tracks[0].name.capacity(),
                1,
            ),
            (
                "inventory.midi_name",
                inventory.midi_clips[0].name.capacity(),
                1,
            ),
            (
                "inventory.audio_name",
                inventory.audio_clips[0].name.capacity(),
                1,
            ),
        ];
        let inventory = Arc::new(inventory);
        let alias = inventory.clone();
        let independent = Arc::new(
            PreparedSessionInventory::prepare_with_media(
                editor.project(),
                &media,
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let strong = Arc::strong_count(&inventory);
        let mut sink = Sink::new(256);
        for value in [&inventory, &alias, &independent] {
            let mut report = Report::new(&mut sink);
            let owner = report.arc(value, "session_inventory").unwrap();
            value
                .visit_source_templates_and_tempo(&mut report, owner)
                .unwrap();
            assert!(report.finish().require_complete().is_err());
        }
        for (name, capacity, stride) in expected {
            assert_eq!(
                sink.measured(name, [0; 2]),
                (capacity, stride, capacity * stride)
            );
            let matching: Vec<_> = sink
                .items
                .iter()
                .filter(|i| i.location.component == name && i.location.indices == [0; 2])
                .collect();
            assert_eq!(matching.len(), 3);
            let owner = |i: usize| {
                let StorageOwner::Shared(owner) = matching[i].owner else {
                    panic!("shared owner")
                };
                owner
            };
            assert!(owner(0).same_allocation(owner(1)));
            assert!(!owner(0).same_allocation(owner(2)));
        }
        assert_eq!(Arc::strong_count(&inventory), strong);
        let item_count = sink.items.len() / 3;
        drop(sink);
        for limit in 0..item_count {
            let mut sink = Sink::new(limit);
            let mut report = Report::new(&mut sink);
            let result = report
                .arc(&inventory, "session_inventory")
                .and_then(|owner| inventory.visit_source_templates_and_tempo(&mut report, owner));
            assert_eq!(result, Err(StorageReportError::SinkCapacity));
            assert_eq!(Arc::strong_count(&inventory), strong);
            // Discard every partial sink; it is not a receipt.
        }
        assert_eq!(inventory.midi_clips[0].name, "clip");
    }
}
