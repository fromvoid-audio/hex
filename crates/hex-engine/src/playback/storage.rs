use super::*;
use crate::storage_report::{
    PlaybackStorageSummary, PlaybackStorageVisitor, Report, StorageOwner, StorageReportError,
};
impl Playback {
    /// Inspect only while exclusively control-owned. A callback-owned Playback
    /// must never be traversed concurrently. This does not allocate, clone owners
    /// or change runtime state. The inline Playback is charged by its containing
    /// allocation; callers charge their actual Box/envelope separately.
    pub fn visit_storage_on_control<'a>(
        &'a mut self,
        sink: &mut impl PlaybackStorageVisitor<'a>,
    ) -> Result<PlaybackStorageSummary, StorageReportError> {
        let Self {
            #[cfg(feature = "instrument-test-support")]
            incomplete_claim_fixture,
            project,
            prepared_revision,
            session_runtime,
            linear_runtime,
            session_unavailable: _,
            position: _,
            performance_frame: _,
            performance_clock_exhausted: _,
            playing: _,
        } = self;
        let mut report = Report::new(sink);
        report.owned_project(project)?;
        if let Some(revision) = prepared_revision {
            report.owned_revision(revision)?;
        }
        if let Some(runtime) = session_runtime {
            report.data(
                StorageOwner::Exclusive,
                "session.box",
                [0; 2],
                1,
                std::mem::size_of::<SessionPlayback>(),
            )?;
            runtime.visit_storage(&mut report)?;
        }
        if let Some(runtime) = linear_runtime {
            report.data(
                StorageOwner::Exclusive,
                "renderer.box",
                [0; 2],
                1,
                std::mem::size_of::<RendererRuntime>(),
            )?;
            report.renderer(runtime)?;
        }
        #[cfg(feature = "instrument-test-support")]
        if *incomplete_claim_fixture {
            report.unknown(
                StorageOwner::Exclusive,
                "fixture.incomplete",
                crate::storage_report::UnknownStorageReason::NestedOwnerContract,
            )?;
        }
        Ok(report.finish())
    }
}
