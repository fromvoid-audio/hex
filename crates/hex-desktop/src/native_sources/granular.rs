//! Control-side granular construction on the existing Session worker.
//! Limits cover listed DATA, not allocator/Arc metadata or native file/thread costs.
use super::*;
use crate::audio_policy::GranularPolicy;
use hex_media::{ResampleKernel, ResamplePlan, ResampleVersion, VerifiedWavReader};
use hex_stream::{
    GranularBankAllowance, GranularSourcePreparationLimits, GranularWorkerBank,
    VerifiedGranularSource,
};

impl DomainSources {
    pub(crate) fn prepare_for_snapshot(
        &self,
        snapshot: &CanonicalAudioSourceSnapshot,
        config: hex_stream::StreamConfig,
        policy: GranularPolicy,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<
        (
            Vec<hex_stream::SourceDefinition>,
            Option<GranularWorkerBank>,
        ),
        String,
    > {
        if cancelled() {
            return Err("source preparation cancelled".into());
        }
        let definitions = self.definitions_for_snapshot(snapshot)?;
        let bank = self.prepare_granular_bank(
            snapshot
                .required_granular_roots(self.catalog_generation)
                .map(|result| result.map_err(|e| e.to_string())),
            config,
            policy,
            cancelled,
        )?;
        Ok((definitions, bank))
    }
    pub(crate) fn prepare_for_files(
        &self,
        input: &hex_session::service::ResidentFileInput<'_>,
        config: hex_stream::StreamConfig,
        policy: GranularPolicy,
        cancelled: impl FnMut() -> bool,
    ) -> Result<
        (
            Vec<hex_stream::SourceDefinition>,
            Option<GranularWorkerBank>,
        ),
        String,
    > {
        let definitions = self.definitions_for_files(input, config.max_kernel_table_bytes)?;
        // A lazy independent root copy coexists with at most one admitted bank.
        // Reserve its complete payload before cloning; the common constructor
        // separately admits subsequent reader/kernel/processing construction.
        let copy_limit = policy
            .max_construction_bytes
            .saturating_sub(policy.max_bank_bytes)
            .min(config.max_kernel_table_bytes);
        if input.granulars.iter().any(|requirement| {
            requirement
                .owned_root_bytes()
                .is_none_or(|bytes| bytes > copy_limit)
        }) {
            return Err("granular root copy reservation exceeded".into());
        }
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let roots = input.granulars.iter().map(|requirement| {
            let (file, _) = Self::granular_for(input, requirement, self.catalog_generation)?;
            let raw = file
                .source_metadata(requirement.asset(), self.catalog_generation)
                .map_err(|e| e.to_string())?;
            let root = requirement
                .try_owned_root(&raw, &cancel)
                .map_err(|e| e.to_string())?;
            Ok((file, root))
        });
        let bank = self.prepare_granular_bank(roots, config, policy, cancelled)?;
        Ok((definitions, bank))
    }
    fn prepare_granular_bank<'a>(
        &self,
        roots: impl Iterator<
            Item = Result<
                (
                    &'a VerifiedAudioFile,
                    hex_engine::audio_transform::StreamedGranularRoot,
                ),
                String,
            >,
        >,
        config: hex_stream::StreamConfig,
        policy: GranularPolicy,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Option<GranularWorkerBank>, String> {
        if cancelled() {
            return Err("source preparation cancelled".into());
        }
        let count = self
            .members
            .iter()
            .filter(|m| m.metadata.key().granular_identity().is_some())
            .count();
        if count == 0 {
            return Ok(None);
        }
        let slots = count
            .checked_mul(std::mem::size_of::<VerifiedGranularSource>())
            .ok_or("granular source size overflow")?;
        if count > config.max_sources
            || slots > policy.max_bank_bytes
            || slots > policy.max_construction_bytes
        {
            return Err("granular source reservation exceeded".into());
        }
        let mut sources: Vec<VerifiedGranularSource> = Vec::new();
        sources
            .try_reserve_exact(count)
            .map_err(|_| "could not reserve granular sources")?;
        let allowance = GranularBankAllowance {
            max_sources: count,
            max_data_bytes: policy.max_bank_bytes,
        };
        let mut retained = slots;
        for requirement in roots {
            if cancelled() {
                return Err("source preparation cancelled".into());
            }
            let (file, root) = requirement.map_err(|e| e.to_string())?;
            if sources
                .iter()
                .any(|source| source.metadata().key() == root.metadata().key())
            {
                continue;
            }
            if !self.contains_member(file, root.metadata()) {
                return Err("granular source facts differ".into());
            }
            let raw = file
                .source_metadata(root.metadata().key().asset(), self.catalog_generation)
                .map_err(|e| e.to_string())?;
            let mut limits = GranularSourcePreparationLimits {
                source_scratch_frames: config.source_scratch_frames,
                checkpoint_entries: 8,
                checkpoint_budget: 16 * 1024 * 1024,
                max_processing_bytes: 32 * 1024 * 1024,
                max_construction_peak_bytes: 0,
            };
            limits.max_construction_peak_bytes = limits
                .construction_processing_bound(raw.project_rate())
                .map_err(|e| format!("granular processing refused: {e:?}"))?;
            let mut reader_limits = file.reader_limits();
            reader_limits.max_manifest_bytes = reader_limits
                .max_manifest_bytes
                .min(config.max_manifest_bytes);
            // Reserve a possible full re-verification manifest and a maximum conversion
            // kernel before either constructor. Captured aliases are conservatively charged.
            let reader_and_kernel = file
                .path()
                .as_os_str()
                .len()
                .checked_add(reader_limits.max_manifest_bytes)
                .and_then(|n| n.checked_add(hex_media::VerifiedWavManifest::owner_payload_bytes()))
                .and_then(|n| n.checked_add(ResampleKernel::owner_payload_bytes()))
                .and_then(|n| n.checked_add(ResampleKernel::max_table_payload_bytes()))
                .and_then(|n| n.checked_add(root.backing_bytes()?))
                .ok_or("granular construction size overflow")?;
            if file.path().as_os_str().len() > config.max_catalog_path_bytes
                || retained
                    .checked_add(reader_and_kernel)
                    .and_then(|n| n.checked_add(limits.max_processing_bytes))
                    .is_none_or(|n| n > policy.max_bank_bytes)
                || retained
                    .checked_add(reader_and_kernel)
                    .and_then(|n| n.checked_add(limits.max_construction_peak_bytes))
                    .is_none_or(|n| n > policy.max_construction_bytes)
            {
                return Err("granular source reservation exceeded".into());
            }
            let reader = match VerifiedWavReader::open_verified_with_cancel(
                file.path(),
                reader_limits,
                file.verified_manifest().clone(),
                &mut cancelled,
            ) {
                Ok(reader) => reader,
                Err(hex_media::ImportError::SourceChanged) => {
                    VerifiedWavReader::open_with_cancel(file.path(), reader_limits, &mut cancelled)
                        .map_err(|e| e.to_string())?
                }
                Err(error) => return Err(error.to_string()),
            };
            let version = ResampleVersion::for_conversion(raw.conversion_version())
                .map_err(|e| e.to_string())?;
            let kernel =
                ResampleKernel::with_version(raw.source_rate(), raw.project_rate(), version)
                    .map_err(|e| e.to_string())?;
            let plan = ResamplePlan::with_kernel(
                kernel,
                raw.source_frames(),
                raw.source_frames(),
                raw.source_playback_frames(),
                hex_engine::STREAM_PAGE_FRAMES,
            )
            .map_err(|e| e.to_string())?;
            let source = VerifiedGranularSource::prepare_from_reader_on_control(
                raw,
                root,
                reader,
                plan,
                limits,
                &mut cancelled,
            )
            .map_err(|e| format!("granular source preparation refused: {e:?}"))?;
            sources.push(source);
            // Admission is allocation-free and measures actual capacities after each source.
            let bank = GranularWorkerBank::try_new_on_control(sources, allowance)
                .map_err(|e| format!("granular bank refused: {e:?}"))?;
            retained = bank.data().bytes().ok_or("granular bank size overflow")?;
            sources = bank.into_sources();
        }
        if sources.len() != count || cancelled() {
            return Err("granular source preparation incomplete or cancelled".into());
        }
        let bank = GranularWorkerBank::try_new_on_control(sources, allowance)
            .map_err(|e| format!("granular bank refused: {e:?}"))?;
        Ok(Some(bank))
    }
}
