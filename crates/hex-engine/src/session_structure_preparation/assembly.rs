use super::*;
use crate::session_inventory::{
    PreparedSessionCell, PreparedSessionCellContent, audio_assembly::SceneRangePlan,
};
impl SessionStructureConstruction {
    pub(crate) fn assembly_scene_targets(
        &self,
    ) -> impl Iterator<Item = crate::PreparedSessionScene> + '_ {
        self.scenes.iter().map(|r| crate::PreparedSessionScene {
            id: r.value.id,
            name: String::new(),
        })
    }
    pub(crate) fn assembly_track_targets(
        &self,
    ) -> impl Iterator<Item = crate::PreparedSessionTrack> + '_ {
        self.tracks.iter().map(|r| crate::PreparedSessionTrack {
            id: r.value.id,
            name: String::new(),
            gain: r.value.gain,
            balance: r.value.balance,
            muted: r.value.muted,
            soloed: r.value.soloed,
            work: r.value.work,
        })
    }
    pub(crate) fn preflight_final_names(
        &self,
        scenes: &[crate::PreparedSessionScene],
        tracks: &[crate::PreparedSessionTrack],
    ) -> Result<(), AudioStorageError> {
        if scenes.len() != self.scenes.len()
            || tracks.len() != self.tracks.len()
            || scenes.iter().zip(&self.scenes).any(|(target, row)| {
                target.id != row.value.id
                    || target.name.capacity() != 0
                    || row.disposal != Disposal::Unstarted
            })
            || tracks.iter().zip(&self.tracks).any(|(target, row)| {
                target.id != row.value.id
                    || target.name.capacity() != 0
                    || row.disposal != Disposal::Unstarted
                    || target.gain != row.value.gain
                    || target.balance != row.value.balance
                    || target.muted != row.value.muted
                    || target.soloed != row.value.soloed
                    || target.work != row.value.work
                    || row.value.work != crate::SessionWorkEnvelope::default()
            })
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(crate) fn assembly_cells(
        &self,
        sources: &SessionSourceConstruction,
    ) -> Result<impl Iterator<Item = PreparedSessionCell> + '_, AudioStorageError> {
        self.validate(sources, false)?;
        Ok(self.cells.iter().map(|row| PreparedSessionCell {
            track: row.track,
            scene: row.scene,
            content: row.audio.as_ref().map_or_else(
                || {
                    row.midi.map_or(
                        PreparedSessionCellContent::Stop,
                        PreparedSessionCellContent::Midi,
                    )
                },
                |binding| PreparedSessionCellContent::Audio(binding.clip_id()),
            ),
        }))
    }
    pub(crate) fn assembly_indices(&self) -> &[usize] {
        &self.scene_index
    }
    pub(crate) fn assembly_ranges(&self) -> impl Iterator<Item = SceneRangePlan> + '_ {
        self.ranges.iter().map(|r| SceneRangePlan {
            id: r.id,
            start: r.start,
            end: r.end,
        })
    }
    pub(crate) fn commit_scene(
        &mut self,
        index: usize,
        target: &mut [crate::PreparedSessionScene],
    ) -> bool {
        let Some(row) = self.scenes.get_mut(index) else {
            return false;
        };
        let Some(target) = target.get_mut(index) else {
            return false;
        };
        if target.id != row.value.id
            || target.name.capacity() != 0
            || row.disposal != Disposal::Unstarted
        {
            return false;
        }
        target.name = mem::take(&mut row.value.name);
        row.disposal = Disposal::Transferred;
        true
    }
    pub(crate) fn commit_track(
        &mut self,
        index: usize,
        target: &mut [crate::PreparedSessionTrack],
    ) -> bool {
        let Some(row) = self.tracks.get_mut(index) else {
            return false;
        };
        let Some(target) = target.get_mut(index) else {
            return false;
        };
        if target.id != row.value.id
            || target.name.capacity() != 0
            || row.disposal != Disposal::Unstarted
        {
            return false;
        }
        target.name = mem::take(&mut row.value.name);
        row.disposal = Disposal::Transferred;
        true
    }
}
