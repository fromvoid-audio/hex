//! Narrow private source-to-final transfer; no mutable row/Vec owner escapes.
use super::*;
use crate::session_inventory::audio_assembly::{AudioTarget, ClipPlan};
impl SessionSourceConstruction {
    pub(crate) fn assembly_plans(&self) -> impl Iterator<Item = ClipPlan> + '_ {
        self.cells.iter().enumerate().map(|(index, row)| ClipPlan {
            active: row.active,
            source_row: index,
            bank_source: self.cache[row.cache].source,
            handle: row.handle,
            clip: row.clip,
            track: row.track,
            scene: row.scene,
            asset: row.asset,
            start: row.start,
            end: row.end,
            loop_range: row.loop_range,
            gain: row.gain,
        })
    }
    pub(crate) fn validate_assembly_row(
        &self,
        plan: &ClipPlan,
        bank: &OwnedAudioInputs,
    ) -> Result<(), AudioStorageError> {
        let row = self
            .cells
            .get(plan.source_row)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let cache = self
            .cache
            .get(row.cache)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let (asset, original) = bank
            .sources
            .get(plan.bank_source)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let owner = row
            .owner
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?;
        if row.disposal != Disposal::Unstarted
            || row.handle != plan.handle
            || owner.handle != plan.handle
            || row.active != plan.active
            || row.clip != plan.clip
            || row.track != plan.track
            || row.scene != plan.scene
            || row.asset != plan.asset
            || row.start != plan.start
            || row.end != plan.end
            || row.loop_range != plan.loop_range
            || row.gain != plan.gain
            || *asset != plan.asset
            || cache.source != plan.bank_source
            || !crate::audio_input_storage::same(owner, original)
            || crate::audio_input_storage::parts(owner)?
                != crate::audio_input_storage::parts(original)?
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(crate) fn commit_assembly_row(
        &mut self,
        plan: &ClipPlan,
        target: &mut AudioTarget,
    ) -> bool {
        // Whole-batch borrowing preflight already checked indices, exact handles
        // and vacancies. No row structure changes during the fixed-move loop.
        let Some(row) = self.cells.get_mut(plan.source_row) else {
            return false;
        };
        if !target.vacant()
            || row.handle != plan.handle
            || row.disposal != Disposal::Unstarted
            || row.owner.is_none()
        {
            return false;
        }
        if let Some(owner) = row.owner.take() {
            let name = mem::take(&mut row.name);
            target.commit(owner, name);
            row.disposal = Disposal::Transferred;
            return true;
        }
        false
    }
}
