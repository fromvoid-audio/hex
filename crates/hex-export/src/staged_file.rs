//! A concrete persistent output File, never a renderer/runtime custodian.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Writing,
    PublicationStarted,
    Published,
}

/// Persistent staged PCM16 output for a host's productive control-work loop.
/// Copy a completed audio block while holding its short custodian loan, release
/// that loan, append here, then separately acknowledge the original block.
/// This value owns no Playback/Renderer, synchronization primitive or permit.
/// The host must retain its own written-but-not-yet-acknowledged correlation and
/// charge this actual body/paths/policy alongside all runtime/queue/copy buffers.
pub struct StagedWavFile {
    writer: Option<buffered_file::BufferedWavFile>,
    destination: PathBuf,
    temporary_path: PathBuf,
    publication: Publication,
    sample_rate: u32,
    frames: u64,
    data_bytes: u32,
    phase: Phase,
}
impl StagedWavFile {
    /// Create only a new scratch file; the destination is unchanged until publish.
    /// A replacement requires the existing concrete confirmed-save policy.
    pub fn begin_on_control(
        path: &Path,
        sample_rate: u32,
        frames: u64,
        publication: &Publication,
    ) -> Result<Self, ExportError> {
        Self::begin_encoded(path, sample_rate, frames, publication, WavEncoding::Pcm16)
    }

    pub(super) fn begin_encoded(
        path: &Path,
        sample_rate: u32,
        frames: u64,
        publication: &Publication,
        encoding: WavEncoding,
    ) -> Result<Self, ExportError> {
        let (data_bytes, byte_rate) =
            validate_export(path, sample_rate, frames, publication, encoding)?;
        // Own paths/policy before creating File: no File escapes an allocation
        // failure while assembling this persistent destination.
        let destination = path.to_owned();
        let publication = publication.clone();
        let (file, temporary_path) = create_temporary(path)?;
        let writer = match buffered_file::BufferedWavFile::new(
            file,
            sample_rate,
            frames,
            data_bytes,
            byte_rate,
            encoding,
        ) {
            Ok(writer) => writer,
            Err(error) => {
                let _ = fs::remove_file(&temporary_path);
                return Err(error);
            }
        };
        Ok(Self {
            writer: Some(writer),
            destination,
            temporary_path,
            publication,
            sample_rate,
            frames,
            data_bytes,
            phase: Phase::Writing,
        })
    }

    /// Actual owned path backings only. The caller must separately visit the
    /// real enclosing slot body/capacity, including the inline encoding buffer.
    /// This is not a renderer/source/whole-application storage declaration.
    pub fn visit_path_storage_on_control<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        ledger.borrow_metadata_path_backing_on_control(&self.destination)?;
        ledger.borrow_metadata_path_backing_on_control(&self.temporary_path)?;
        self.publication.visit_path_storage_on_control(ledger)
    }

    /// Nonowning progress only; it does not acknowledge a runtime block.
    pub fn written_frames(&self) -> u64 {
        self.writer
            .as_ref()
            .map_or(self.frames, |writer| writer.written())
    }

    /// No family/custodian guard may remain held over this concrete File I/O.
    /// Wrong offsets reject before writing; I/O/nonfinite failures are sticky.
    pub fn append_on_control(
        &mut self,
        expected_offset: u64,
        block: &[hex_engine::Frame],
    ) -> Result<(), ExportError> {
        if self.phase != Phase::Writing {
            return Err(ExportError::Invalid("staged wav is not writable"));
        }
        self.writer
            .as_mut()
            .ok_or(ExportError::Invalid("staged wav writer is absent"))?
            .append(expected_offset, block)
    }

    pub(super) fn render(&mut self, renderer: &mut impl BlockRenderer) -> Result<(), ExportError> {
        let writer = self
            .writer
            .as_mut()
            .ok_or(ExportError::Invalid("staged wav writer is absent"))?;
        write_wav(writer, self.frames, renderer)
    }

    /// Flush/sync before the existing race-safe create/replace publication.
    /// Any publication attempt is irreversible for this record: failure/panic
    /// cannot cause a second exchange or deletion of a displaced user file.
    pub fn publish_on_control(&mut self) -> Result<ExportReport, ExportError> {
        self.publish_with(|| {})
    }

    fn publish_with(
        &mut self,
        before_exchange: impl FnOnce(),
    ) -> Result<ExportReport, ExportError> {
        if self.phase != Phase::Writing {
            return Err(ExportError::Invalid(
                "staged wav publication already attempted",
            ));
        }
        self.writer
            .as_mut()
            .ok_or(ExportError::Invalid("staged wav writer is absent"))?
            .finish()?;
        self.phase = Phase::PublicationStarted;
        drop(self.writer.take());
        let report = publish_staged(
            &self.destination,
            &self.temporary_path,
            self.sample_rate,
            self.frames,
            self.data_bytes,
            &self.publication,
            before_exchange,
        )?;
        self.phase = Phase::Published;
        Ok(report)
    }
}
impl Drop for StagedWavFile {
    fn drop(&mut self) {
        if self.phase == Phase::Writing {
            drop(self.writer.take());
            // Only our unpublished scratch file. After publication starts it
            // may instead hold another writer's displaced file: never delete it.
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("hex-staged-file-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn persistent_file_chunks_publish_once_and_reject_duplicate_offsets() {
        let dir = Scratch::new();
        let path = dir.0.join("result.wav");
        let mut file =
            StagedWavFile::begin_on_control(&path, 48_000, 3, &Publication::CreateNew).unwrap();
        let temporary = file.temporary_path.clone();
        assert!(!path.exists());
        file.append_on_control(0, &[[0.5, -0.5]]).unwrap();
        assert_eq!(file.written_frames(), 1);
        assert!(file.append_on_control(0, &[[0.5, -0.5]]).is_err());
        assert!(file.publish_on_control().is_err());
        assert!(!path.exists());
        assert!(temporary.exists());
        file.append_on_control(1, &[[1.0, -1.0]; 2]).unwrap();
        let report = file.publish_on_control().unwrap();
        assert_eq!(report.frames, 3);
        assert_eq!(report.data_bytes, 12);
        assert_eq!(
            &fs::read(&path).unwrap()[44..],
            &[0, 64, 0, 192, 255, 127, 1, 128, 255, 127, 1, 128]
        );
        assert!(!temporary.exists());
        assert!(file.publish_on_control().is_err());
        assert!(file.append_on_control(3, &[[0.0; 2]]).is_err());
        drop(file);
        assert_eq!(fs::metadata(path).unwrap().len(), 56);
    }

    #[test]
    fn dropped_or_failed_unpublished_output_never_changes_confirmed_destination() {
        let dir = Scratch::new();
        let path = dir.0.join("existing.wav");
        fs::write(&path, b"original").unwrap();
        let policy = Publication::confirmed_save(&path).unwrap();
        let temporary;
        {
            let mut file = StagedWavFile::begin_on_control(&path, 48_000, 1, &policy).unwrap();
            temporary = file.temporary_path.clone();
            assert!(file.append_on_control(0, &[[f32::INFINITY, 0.0]]).is_err());
            assert!(file.publish_on_control().is_err());
            assert_eq!(fs::read(&path).unwrap(), b"original");
        }
        assert!(!temporary.exists());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        // Invalid preconstruction and known collision create no scratch file.
        assert!(StagedWavFile::begin_on_control(&path, 0, 1, &policy).is_err());
        assert!(
            StagedWavFile::begin_on_control(&path, 48_000, 1, &Publication::CreateNew).is_err()
        );
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn late_create_collision_is_preserved_and_cannot_retry_publication() {
        let dir = Scratch::new();
        let path = dir.0.join("raced.wav");
        let mut file =
            StagedWavFile::begin_on_control(&path, 48_000, 1, &Publication::CreateNew).unwrap();
        let temporary = file.temporary_path.clone();
        file.append_on_control(0, &[[0.0; 2]]).unwrap();
        fs::write(&path, b"other writer").unwrap();
        assert!(
            matches!(file.publish_on_control(), Err(ExportError::Io { source, .. }) if source.kind() == io::ErrorKind::AlreadyExists)
        );
        assert!(file.publish_on_control().is_err());
        drop(file);
        assert_eq!(fs::read(&path).unwrap(), b"other writer");
        assert!(!temporary.exists());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn exchanged_race_preserves_displaced_file_even_after_record_drops() {
        let dir = Scratch::new();
        let path = dir.0.join("existing.wav");
        let saved = dir.0.join("original-saved.wav");
        fs::write(&path, b"original").unwrap();
        let policy = Publication::confirmed_save(&path).unwrap();
        let mut file = StagedWavFile::begin_on_control(&path, 48_000, 1, &policy).unwrap();
        let temporary = file.temporary_path.clone();
        file.append_on_control(0, &[[0.0; 2]]).unwrap();
        let result = file.publish_with(|| {
            fs::rename(&path, &saved).unwrap();
            fs::write(&path, b"other writer").unwrap();
        });
        assert!(matches!(
            result,
            Err(ExportError::PublicationConflict { .. })
        ));
        assert!(file.publish_on_control().is_err());
        drop(file);
        assert_eq!(fs::read(saved).unwrap(), b"original");
        assert_eq!(fs::read(temporary).unwrap(), b"other writer");
        assert_eq!(&fs::read(path).unwrap()[..4], b"RIFF");
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn publication_panic_keeps_record_nonretryable_and_preserves_both_paths() {
        let dir = Scratch::new();
        let path = dir.0.join("existing.wav");
        fs::write(&path, b"original").unwrap();
        let policy = Publication::confirmed_save(&path).unwrap();
        let mut file = StagedWavFile::begin_on_control(&path, 48_000, 1, &policy).unwrap();
        let temporary = file.temporary_path.clone();
        file.append_on_control(0, &[[0.0; 2]]).unwrap();
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            file.publish_with(|| panic!("injected staged publication"))
        }))
        .unwrap_err();
        assert_eq!(
            payload.downcast_ref::<&str>(),
            Some(&"injected staged publication")
        );
        assert!(file.phase == Phase::PublicationStarted);
        assert!(file.publish_on_control().is_err());
        assert!(file.append_on_control(1, &[[0.0; 2]]).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        drop(file);
        assert!(
            temporary.exists(),
            "publication-pending scratch remains recoverable"
        );
        assert_eq!(fs::read(path).unwrap(), b"original");
    }
}
