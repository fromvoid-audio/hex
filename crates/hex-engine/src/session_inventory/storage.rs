use super::PreparedSessionInventory;
use crate::storage_report::{PlaybackStorageVisitor, Report, StorageReportError};
impl PreparedSessionInventory {
    /// All other inventory owners remain explicitly Unknown in the caller.
    pub(crate) fn visit_source_templates<'a, V: PlaybackStorageVisitor<'a>>(
        &'a self,
        report: &mut Report<'a, '_, V>,
    ) -> Result<(), StorageReportError> {
        for clip in &self.midi_clips {
            let owner = report.arc(&clip.source, "session_midi_source_template")?;
            clip.source.visit_storage(report, owner)?;
        }
        Ok(())
    }
}
