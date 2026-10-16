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
                    hex_engine::STREAM_PAGE_FRAMES,
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
        Ok(Self {
            stream,
            readiness,
            publication: AudioCandidatePublicationReservation {
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
            },
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
