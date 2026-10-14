use super::PreparedSessionInventory;
use crate::storage_report::{PlaybackStorageVisitor, Report, StorageOwner, StorageReportError};
impl PreparedSessionInventory {
    /// All other inventory owners remain explicitly Unknown in the caller.
    pub(crate) fn visit_source_templates_and_tempo<'a, V: PlaybackStorageVisitor<'a>>(
        &'a self,
        report: &mut Report<'a, '_, V>,
        owner: StorageOwner<'a>,
    ) -> Result<(), StorageReportError> {
        self.timing.visit_storage(report, owner)?;
        for clip in &self.midi_clips {
            let owner = report.arc(&clip.source, "session_midi_source_template")?;
            clip.source.visit_storage(report, owner)?;
        }
        Ok(())
    }
}
