//! Finite caller reservations for this native document pipeline, not RSS limits.
//! Three cells mean one active, one queued successor and one construction. N2
//! remains the actual shared source-domain occupancy, including failed archives.
use hex_audio::AudioCandidatePublicationReservation;
use hex_session::SourcePlanReservation;

#[derive(Clone, Copy)]
pub(super) struct AudioPolicy {
    pub(super) stream: hex_stream::StreamConfig,
    pub(super) readiness: hex_stream::StreamReadinessReservation,
    pub(super) publication: AudioCandidatePublicationReservation,
    pub(super) resident_native: hex_session::ResidentNativeReservation,
    pub(super) resident_publication: AudioCandidatePublicationReservation,
    pub(super) control_backing_quote: usize,
    pub(super) legacy_hold_bytes: usize,
}
pub(super) const RUNTIME_CELLS: usize = 3;
pub(super) const MAX_RESIDENT_FRAMES: u64 = 8_000_000;
impl AudioPolicy {
    pub(super) fn source_reservation() -> SourcePlanReservation {
        let stream = hex_stream::StreamConfig::initial();
        SourcePlanReservation {
            reader: hex_media::VerifiedWavReaderLimits::new(
                hex_media::WavReaderLimits::new(
                    64 * 1024 * 1024,
                    8_000_000,
                    32,
                    // Output pages are not source windows: downsampling also
                    // needs the resampler halo. The worker already reserves
                    // and accounts this bounded source scratch before decode.
                    stream.source_scratch_frames,
                ),
                stream.max_manifest_bytes,
            ),
            max_retained_sources: stream.max_sources,
            max_path_bytes: stream.max_catalog_path_bytes,
            max_manifest_bytes: stream.max_manifest_bytes,
        }
    }
    pub(super) fn new() -> Result<Self, String> {
        let stream = hex_stream::StreamConfig::initial();
        let readiness = hex_stream::StreamReadinessReservation {
            extents_per_role: stream
                .page_buffers
                .checked_sub(1)
                .ok_or("invalid page capacity")?,
        };
        let sources = Self::source_reservation();
        sources.validate().map_err(|e| e.to_string())?;
        let quote =
            hex_engine::AudioCandidateReservationQuote::for_supported_limits(stream.max_sources)
                .map_err(|e| format!("audio reservation refused: {e:?}"))?;
        let input = usize::try_from(MAX_RESIDENT_FRAMES)
            .ok()
            .and_then(|n| n.checked_mul(std::mem::size_of::<hex_engine::Frame>()))
            .ok_or("resident reservation overflow")?;
        let graph = quote
            .storage_reservation(input)
            .map_err(|e| format!("audio reservation refused: {e:?}"))?;
        // Reuse the actual no-owner constructor quote for transport/disposal and
        // fixed worker backing, then reserve the existing variable source caps.
        // Actual definitions are quoted again by Root BEFORE any worker start.
        let fixed = hex_stream::PendingStreamDomain::preparation_bound(
            stream,
            &[],
            stream.max_sources,
            Some(readiness),
        )
        .map_err(|e| format!("source reservation refused: {e:?}"))?;
        let variable = [
            stream.max_catalog_path_bytes,
            stream.max_manifest_bytes,
            stream.max_kernel_table_bytes,
            stream.max_reader_scratch_bytes,
            stream
                .max_sources
                .checked_mul(std::mem::size_of::<hex_media::VerifiedWavReader>())
                .ok_or("reader reservation overflow")?,
        ]
        .into_iter()
        .try_fold(0usize, |n, b| n.checked_add(b))
        .ok_or("source reservation overflow")?;
        let domain_data = fixed
            .retained_data_bytes
            .checked_add(variable)
            .ok_or("source reservation overflow")?;
        let domain_peak = fixed
            .construction_peak_data_bytes
            .checked_add(variable)
            .ok_or("source reservation overflow")?;
        let retained = graph
            .max_prepared_data_bytes
            .checked_mul(RUNTIME_CELLS)
            .and_then(|n| n.checked_add(domain_data.checked_mul(2)?))
            .ok_or("audio reservation overflow")?;
        let peak = graph
            .max_construction_data_bytes
            .checked_mul(RUNTIME_CELLS)
            .and_then(|n| n.checked_add(domain_peak.checked_mul(2)?))
            .ok_or("audio reservation overflow")?;
        let handles = quote
            .handle_rows
            .checked_mul(RUNTIME_CELLS)
            .and_then(|n| {
                n.checked_add(
                    stream
                        .max_sources
                        .checked_add(fixed.shared_handle_bound)?
                        .checked_mul(2)?,
                )
            })
            // An additional supported-constructor ceiling conservatively
            // reserves the native root's private channel/carrier handles. This
            // is deliberately not an exact fixed-handle count: actual closure
            // and multiplicity still require the native constructor witness.
            .and_then(|n| {
                n.checked_add(
                    hex_engine::AudioCandidateReservationQuote::for_supported_limits(0)
                        .ok()?
                        .handle_rows,
                )
            })
            .ok_or("audio handle reservation overflow")?;
        let publication = AudioCandidatePublicationReservation {
            runtime_slots: RUNTIME_CELLS,
            archive_slots: RUNTIME_CELLS,
            receipt_rows: RUNTIME_CELLS,
            member_rows: quote
                .member_rows
                .checked_mul(RUNTIME_CELLS)
                .ok_or("audio member overflow")?,
            transfer_rows: quote
                .transfer_rows
                .checked_mul(RUNTIME_CELLS)
                .ok_or("audio transfer overflow")?,
            data_rows: quote.data_rows,
            handle_rows: handles,
            max_retained_data_bytes: retained,
            max_construction_peak_data_bytes: peak,
        };
        let native_quote =
            hex_engine::AudioCandidateReservationQuote::for_supported_resident_limits()
                .map_err(|e| format!("resident constructor quote refused: {e:?}"))?;
        let native_graph = native_quote
            .storage_reservation(input)
            .map_err(|e| format!("resident constructor reservation refused: {e:?}"))?;
        let resident_publication = AudioCandidatePublicationReservation {
            runtime_slots: RUNTIME_CELLS,
            archive_slots: RUNTIME_CELLS,
            receipt_rows: RUNTIME_CELLS,
            member_rows: native_quote
                .member_rows
                .checked_mul(RUNTIME_CELLS)
                .ok_or("resident member overflow")?,
            transfer_rows: native_quote
                .transfer_rows
                .checked_mul(RUNTIME_CELLS)
                .ok_or("resident transfer overflow")?,
            data_rows: native_quote.data_rows,
            handle_rows: native_quote
                .handle_rows
                .checked_mul(RUNTIME_CELLS + 1)
                .ok_or("resident handles overflow")?,
            max_retained_data_bytes: native_graph
                .max_prepared_data_bytes
                .checked_mul(RUNTIME_CELLS)
                .ok_or("resident DATA overflow")?,
            max_construction_peak_data_bytes: native_graph
                .max_construction_data_bytes
                .checked_mul(RUNTIME_CELLS)
                .ok_or("resident peak overflow")?,
        };
        let control_backing_quote = resident_publication
            .worker_control_storage_quote()
            .ok_or("native control storage quote overflow")?;
        let legacy_hold_bytes = native_quote
            .metadata_data_bytes
            .checked_add(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
            .and_then(|n| n.checked_add(control_backing_quote))
            .ok_or("legacy native hold overflow")?;
        // One physical PCM allowance is shared by Session/staged/export/native
        // owners. Immutable metadata covers three native roles plus the three
        // Service current/staged/export selections; runtime has its own32MiB
        // per-owner limit, including the independent exporter. No frame cap grows.
        let immutable_roles = RUNTIME_CELLS.checked_mul(2).ok_or("native role overflow")?;
        let runtime_roles = RUNTIME_CELLS.checked_add(1).ok_or("native role overflow")?;
        let native_retained = input
            .checked_add(
                native_quote
                    .metadata_data_bytes
                    .checked_mul(immutable_roles)
                    .ok_or("native metadata overflow")?,
            )
            .and_then(|n| {
                n.checked_add(
                    hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES.checked_mul(runtime_roles)?,
                )
            })
            .and_then(|n| n.checked_add(control_backing_quote.checked_mul(RUNTIME_CELLS)?))
            .ok_or("native retained reservation overflow")?;
        let native_peak = native_retained
            .checked_add(native_quote.compiler_workspace_bytes)
            .and_then(|n| n.checked_add(native_quote.metadata_data_bytes))
            .ok_or("native construction reservation overflow")?;
        let resident_native = hex_session::ResidentNativeReservation {
            max_scan_frames: MAX_RESIDENT_FRAMES,
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: native_quote
                    .data_rows
                    .checked_mul(immutable_roles)
                    .ok_or("native DATA rows overflow")?,
                handle_rows: native_quote
                    .handle_rows
                    .checked_mul(immutable_roles)
                    .ok_or("native handle rows overflow")?,
                root_rows: native_quote
                    .source_capacity
                    .checked_mul(immutable_roles)
                    .and_then(|n| n.checked_add(RUNTIME_CELLS))
                    .ok_or("native root rows overflow")?,
                max_retained_bytes: native_retained,
                max_construction_peak_bytes: native_peak,
            },
        };
        Ok(Self {
            stream,
            readiness,
            publication,
            resident_native,
            resident_publication,
            control_backing_quote,
            legacy_hold_bytes,
        })
    }
    pub(super) fn configure(
        self,
        control: &mut hex_audio::prepared::PublicationControl,
    ) -> Result<(), String> {
        // Publication configures the one shared N2 domain bank as well. The
        // domains-only and publication constructors are alternative opt-ins.
        control.configure_audio_candidate_publication(self.publication)?;
        control.configure_audio_candidate_streaming(self.readiness.extents_per_role)?;
        Ok(())
    }
}
