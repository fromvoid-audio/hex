//! The ONE existing native worker File-slot destination now lives beside its
//! actual Audio continuation. No erased owner or second work table is added.
use super::NativeAudioSink;
use hex_audio::CapturedMainFile;
use hex_engine::{
    AudioStorageError,
    sampler_host_fields::{BodyRef, FileJobFields, FileSlotsFields, VecRef},
};
use hex_session::service::MainControlFileJob;

pub(super) struct NativeMainFileJob {
    pub(super) file: MainControlFileJob,
    pub(super) continuation: Option<CapturedMainFile>,
}
struct NativeFileSlots<'a> {
    slots: &'a Vec<NativeMainFileJob>,
}
impl<'a> FileSlotsFields<'a> for NativeFileSlots<'a> {
    fn borrowed_frame(&self) -> BodyRef<'_> {
        BodyRef::of(self)
    }
    fn slots(&self) -> VecRef<'a> {
        VecRef::of(self.slots)
    }
    fn file(&self, index: usize) -> Result<FileJobFields<'a>, AudioStorageError> {
        let NativeMainFileJob { file, continuation } = self
            .slots
            .get(index)
            .ok_or(AudioStorageError::SourceMismatch)?;
        // A present actual continuation is never omitted as an empty header.
        // Full old+new Root guard/schema composition is still mandatory before
        // production admission; this reserved-slot constituent cannot do it.
        if continuation.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        file.sampler_file_fields_on_control()
    }
}
impl NativeAudioSink {
    pub(super) fn main_file_slots_on_control(&self) -> impl FileSlotsFields<'_> {
        NativeFileSlots {
            slots: &self.main_files,
        }
    }
    /// One retirement turn per ordinary worker poll, including Event pressure.
    /// It performs no render/write/publication; productive turns must use the
    /// same persistent job after the closed constructor and request wiring.
    pub(super) fn poll_main_file_retirement_on_control(&mut self) {
        let count = self.main_file_slots_on_control().slots().len();
        if count == 0 {
            self.main_file_cursor = 0;
            return;
        }
        let index = self.main_file_cursor % count;
        self.main_file_cursor = if index + 1 == count { 0 } else { index + 1 };
        let Some(job) = self.main_files.get(index) else {
            return;
        };
        // Failed File/first-panic custody stays in its actual destination. No
        // retry of a consumed operation or owning destructor is attempted.
        if job.file.failed_incomplete() {
            return;
        }
        let retired = job
            .continuation
            .as_ref()
            .is_some_and(|continuation| continuation.poll_retirement_on_control());
        if retired && job.file.published_on_control() {
            // No native guard survives poll_retirement. Only normal completed
            // jobs are destroyed here; File failure remains quarantined above.
            drop(self.main_files.remove(index));
        }
    }
}
