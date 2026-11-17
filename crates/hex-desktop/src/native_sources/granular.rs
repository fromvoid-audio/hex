//! Shared granular construction on control/source-analysis workers.
//! Limits cover listed DATA, not allocator/Arc metadata or native file/thread costs.
use super::*;
use crate::audio_policy::GranularPolicy;
use hex_stream::GranularWorkerBank;

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
        cancelled: impl FnMut() -> bool,
    ) -> Result<Option<GranularWorkerBank>, String> {
        let count = self
            .members
            .iter()
            .filter(|m| m.metadata.key().granular_identity().is_some())
            .count();
        let roots = roots.map(|result| {
            let (file, root) = result?;
            if !self.contains_member(file, root.metadata()) {
                return Err("granular source facts differ".into());
            }
            Ok((file, root))
        });
        crate::granular_sources::prepare_granular_bank(count, roots, config, policy, cancelled)
    }
}
