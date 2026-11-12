//! Neutral precommit file projection for the ordinary native host. File facts,
//! semantic epochs and catalog generations are distinct; none is a retained
//! reader, domain token, page receipt or native publication authority.
use super::*;

pub struct ResidentFileInput<'a> {
    pub context: hex_engine::AudioSourceContext,
    pub semantic_source_epoch: u64,
    pub files: &'a [(hex_project::AssetId, &'a crate::VerifiedAudioFile)],
}
/// Immutable compiler metadata returned by the native host. Final publication
/// must independently match its actual retained catalog and exact page loan.
#[derive(Clone)]
pub struct ResidentFileMetadata {
    catalog_generation: u64,
    sources: BTreeMap<hex_project::AssetId, Arc<hex_engine::StreamedSourceMetadata>>,
}
impl ResidentFileMetadata {
    pub fn new(
        catalog_generation: u64,
        sources: BTreeMap<hex_project::AssetId, Arc<hex_engine::StreamedSourceMetadata>>,
    ) -> Result<Self, String> {
        if catalog_generation == 0
            || sources.iter().any(|(id, metadata)| {
                metadata.key() != hex_engine::StreamSourceKey::new(catalog_generation, *id)
            })
        {
            return Err("ordinary file catalog metadata differs".into());
        }
        Ok(Self {
            catalog_generation,
            sources,
        })
    }
    pub fn catalog_generation(&self) -> u64 {
        self.catalog_generation
    }
    pub fn sources(
        &self,
    ) -> &BTreeMap<hex_project::AssetId, Arc<hex_engine::StreamedSourceMetadata>> {
        &self.sources
    }
    /// Compare complete captured facts, not just a matching ID/generation.
    /// No file is opened and no authority is minted by this check.
    pub(crate) fn validate_for(&self, input: &ResidentFileInput<'_>) -> Result<(), SessionError> {
        if self.sources.len() != input.files.len() || input.semantic_source_epoch == 0 {
            return Err(SessionError::Invalid(
                "ordinary file source metadata count differs",
            ));
        }
        for (id, file) in input.files {
            let expected = file.source_metadata(*id, self.catalog_generation)?;
            if self
                .sources
                .get(id)
                .is_none_or(|metadata| metadata.as_ref() != &expected)
            {
                return Err(SessionError::Invalid(
                    "ordinary file source metadata differs",
                ));
            }
        }
        Ok(())
    }
}

/// Worker-private immutable source facts paired with one prepared graph. It may
/// accompany an explicitly offline retained graph; it never owns a reader or
/// establishes native submission/readiness, and is never emitted in a UI Event.
/// The capture revision may precede an unchanged compiler alias: publication
/// validates the project generation and actual compiled file dependencies.
/// Existing job/context/fresh-runtime fences retain revision freshness custody.
#[derive(Clone)]
pub struct ResidentFileSet {
    inner: Arc<FileSetFacts>,
}
struct FileSetFacts {
    context: hex_engine::AudioSourceContext,
    semantic_source_epoch: u64,
    files: Vec<(hex_project::AssetId, crate::VerifiedAudioFile)>,
    metadata: ResidentFileMetadata,
}
impl ResidentFileSet {
    pub(crate) fn capture(
        input: ResidentFileInput<'_>,
        metadata: ResidentFileMetadata,
    ) -> Result<Self, SessionError> {
        metadata.validate_for(&input)?;
        if input.files.len() > hex_project::MAX_ASSETS {
            return Err(SessionError::Invalid("ordinary file source count exceeded"));
        }
        let mut files = Vec::new();
        files
            .try_reserve_exact(input.files.len())
            .map_err(|_| SessionError::Invalid("ordinary file source reservation failed"))?;
        files.extend(input.files.iter().map(|(id, file)| (*id, (*file).clone())));
        Ok(Self {
            inner: Arc::new(FileSetFacts {
                context: input.context,
                semantic_source_epoch: input.semantic_source_epoch,
                files,
                metadata,
            }),
        })
    }
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.inner.context
    }
    pub fn semantic_source_epoch(&self) -> u64 {
        self.inner.semantic_source_epoch
    }
    pub fn files(&self) -> &[(hex_project::AssetId, crate::VerifiedAudioFile)] {
        &self.inner.files
    }
    pub fn metadata(&self) -> &ResidentFileMetadata {
        &self.inner.metadata
    }
}
/// Borrowed cancellation and factual sources for the last fallible publication
/// operation. A host may retain facts with an accepted offline graph, but not
/// these borrowed flags. Native queue acceptance still requires actual custody.
pub struct ResidentFilePublication<'a> {
    pub sources: &'a ResidentFileSet,
    pub shutdown: &'a AtomicBool,
    pub cancel: Option<&'a AtomicBool>,
}

/// Stateless control capability executed on the existing bounded seek worker.
/// Independent pages must be joined/dropped before returning. Honor the exact
/// requested seek mode or refuse it: history-preserving preparation must replay
/// through bounded pages, never substitute a fresh-history candidate. Native
/// publication remains separate with its original freshness and resource fences.
pub type ResidentFileSeekPreparer =
    fn(
        ResidentFileSeekRequest<'_>,
    ) -> Result<hex_engine::playback::Playback, hex_engine::ArrangementPrerollError>;

/// Exact captured job inputs. Only the Session seek worker constructs this
/// borrow; file facts grant no native reader, endpoint or readiness custody.
pub struct ResidentFileSeekRequest<'a> {
    pub(crate) spec: &'a crate::arrangement_seek::ArrangementSeekSpec,
    pub(crate) resources: &'a hex_engine::native_device_resources::PreparedDeviceResources,
    pub(crate) sources: &'a ResidentFileSet,
    pub(crate) cancelled: &'a AtomicBool,
}
impl ResidentFileSeekRequest<'_> {
    pub fn spec(&self) -> &crate::arrangement_seek::ArrangementSeekSpec {
        self.spec
    }
    pub fn resources(&self) -> &hex_engine::native_device_resources::PreparedDeviceResources {
        self.resources
    }
    pub fn sources(&self) -> &ResidentFileSet {
        self.sources
    }
    pub fn cancelled(&self) -> &AtomicBool {
        self.cancelled
    }
}
