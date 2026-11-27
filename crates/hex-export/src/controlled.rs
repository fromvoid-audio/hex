//! Control-worker rendering through one canonical linear Arrangement owner.
#[cfg(test)]
mod foreign_linear_tests;
use super::*;
use hex_engine::{Frame, OutputCapture, OutputCaptureId, OutputCapturePlan};
use std::sync::atomic::{AtomicBool, AtomicU8};

/// A cancellation/progress handle belonging to one admitted job. Never reuse it.
#[derive(Clone, Debug, Default)]
pub struct ExportControl {
    state: Arc<AtomicU8>,
    claimed: Arc<AtomicBool>,
    processed: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    bounce_units: Arc<AtomicU64>,
    bounce_item: Option<BounceItem>,
}

const BOUNCE_PROGRESS_UNITS: u64 = 10_000;

#[derive(Clone, Copy, Debug)]
struct BounceItem {
    first_unit: u64,
    end_unit: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportProgress {
    /// Includes discarded preroll frames; completion is a separate job result.
    pub processed_frames: u64,
    pub total_frames: u64,
}

impl ExportControl {
    /// Returns false once publication has begun. That commit boundary cannot
    /// be undone by cancellation; the separate completion result is authoritative.
    pub fn cancel(&self) -> bool {
        match self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(1) => true,
            Err(_) => false,
        }
    }

    /// Identity comparison for an exact generated-media completion.
    pub fn same_job(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::Acquire) == 1
    }

    pub fn progress(&self) -> ExportProgress {
        ExportProgress {
            processed_frames: self.processed.load(Ordering::Acquire),
            total_frames: self.total.load(Ordering::Acquire),
        }
    }

    /// Overall Arrangement bounce progress. Each output gets an equal share
    /// of the bar, filled by its rendered frames (including required preroll).
    /// An output's final step is reserved for staging and preparing its media.
    pub fn bounce_fraction(&self) -> f32 {
        self.bounce_units.load(Ordering::Acquire) as f32 / BOUNCE_PROGRESS_UNITS as f32
    }

    /// Scope the frame counters to one output within a serial bounce job. The
    /// stop/adoption fence and overall progress stay shared with the parent.
    pub fn for_bounce_item(&self, index: usize, count: usize) -> Result<Self, ExportError> {
        if index >= count || count > BOUNCE_PROGRESS_UNITS as usize {
            return Err(ExportError::Invalid("invalid bounce progress item"));
        }
        let unit = |i: usize| i as u64 * BOUNCE_PROGRESS_UNITS / count as u64;
        Ok(Self {
            state: Arc::clone(&self.state),
            claimed: Arc::clone(&self.claimed),
            bounce_units: Arc::clone(&self.bounce_units),
            bounce_item: Some(BounceItem {
                first_unit: unit(index),
                end_unit: unit(index + 1),
            }),
            processed: Arc::new(AtomicU64::new(0)),
            total: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Fill the rest of this output's share once staging and media preparation
    /// succeed. The exact job terminal still owns final success or failure.
    pub fn finish_bounce_item(&self) {
        if let Some(item) = self.bounce_item {
            self.bounce_units
                .fetch_max(item.end_unit, Ordering::Release);
        }
    }

    pub(super) fn check(&self) -> Result<(), ExportError> {
        if self.state.load(Ordering::Acquire) == 1 {
            Err(ExportError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub(super) fn claim(&self) -> Result<(), ExportError> {
        self.check()?;
        if self.claimed.swap(true, Ordering::AcqRel) {
            return Err(ExportError::Invalid(
                "export control belongs to another job",
            ));
        }
        Ok(())
    }

    pub(super) fn add_total(&self, extra: u64) {
        if extra > 0 {
            self.total.fetch_add(extra, Ordering::Release);
        }
    }

    pub(super) fn advance(&self, count: usize) {
        let processed = self.processed.fetch_add(count as u64, Ordering::Release) + count as u64;
        if let Some(item) = self.bounce_item {
            let total = self.total.load(Ordering::Relaxed);
            let units = (item.end_unit - item.first_unit - 1)
                .saturating_mul(processed.min(total))
                .checked_div(total)
                .unwrap_or(0);
            self.bounce_units
                .fetch_max(item.first_unit + units, Ordering::Release);
        }
    }

    /// Seal a prepared generated-media transaction immediately before its
    /// infallible adoption. Cancellation can no longer win after this boundary.
    pub fn seal_prepared_adoption(&self) -> Result<(), ExportError> {
        self.begin_publication()
    }

    pub(super) fn begin_publication(&self) -> Result<(), ExportError> {
        match self
            .state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok(()),
            Err(1) => Err(ExportError::Cancelled),
            Err(_) => Err(ExportError::Invalid("export control was already published")),
        }
    }
}

/// Explicit output scope. Capture sums exclude Main/master processing and keep
/// the full contributor graph, including unselected contributors to Returns.
#[derive(Clone, Debug)]
pub enum ExportSignal {
    Main,
    CapturedMix(Vec<OutputCaptureId>),
    Stems(Vec<super::Stem>),
}

/// Conservative control-worker export scratch, separate from the engine's checked linear
/// runtime allowance. Includes exporter block/IO buffers, bounded capture-plan/selection backing,
/// and actual caller-owned selection/name capacities plus the bounded stem working copy. Engine
/// runtime/source/factory payloads, paths, filesystem/platform storage and the control stack are
/// excluded: this is neither whole-graph allocation provenance nor a source-frame reservation.
/// Invalid counts/names and byte overflow refuse before the job captures/distributes resources.
pub fn control_storage_allowance(signal: Option<&ExportSignal>) -> Result<usize, ExportError> {
    let overflow = || ExportError::Invalid("export scratch allowance overflows");
    let block = FRAMES_PER_BLOCK
        .checked_mul(std::mem::size_of::<Frame>())
        .ok_or_else(overflow)?;
    let encoded_block = FRAMES_PER_BLOCK * super::format::MAX_FRAME_BYTES;
    let mut bytes = block
        .checked_add(encoded_block)
        .and_then(|bytes| {
            bytes.checked_add(
                std::mem::size_of::<ControlledRenderer>() - std::mem::size_of::<SessionPlayback>(),
            )
        })
        .ok_or_else(overflow)?;
    // Five shared scalar controls; conservative Arc headers/alignment, never source payloads.
    bytes = bytes
        .checked_add(
            5 * (2 * std::mem::size_of::<usize>()
                + std::mem::size_of::<AtomicU64>()
                + std::mem::align_of::<AtomicU64>()
                - 1),
        )
        .ok_or_else(overflow)?;
    let (count, selection, stems) = match signal {
        None | Some(ExportSignal::Main) => (0, 0, None),
        Some(ExportSignal::CapturedMix(ids)) => {
            if ids.is_empty() || ids.len() > hex_engine::MAX_OUTPUT_CAPTURES {
                return Err(ExportError::Invalid(
                    "invalid export output selection count",
                ));
            }
            (
                ids.len(),
                ids.capacity()
                    .checked_mul(std::mem::size_of::<OutputCaptureId>())
                    .ok_or_else(overflow)?,
                None,
            )
        }
        Some(ExportSignal::Stems(stems)) => {
            if stems.is_empty()
                || stems.len() > hex_engine::MAX_OUTPUT_CAPTURES
                || stems.iter().any(|stem| stem.name.len() > 4096)
            {
                return Err(ExportError::Invalid("invalid stem selection count"));
            }
            let names = stems.iter().try_fold(0usize, |bytes, stem| {
                bytes.checked_add(stem.name.capacity()).ok_or_else(overflow)
            })?;
            let selection = stems
                .capacity()
                .checked_mul(std::mem::size_of::<super::Stem>())
                .and_then(|bytes| bytes.checked_add(names))
                .ok_or_else(overflow)?;
            (stems.len(), selection, Some(stems))
        }
    };
    bytes = bytes.checked_add(selection).ok_or_else(overflow)?;
    let writers = if stems.is_some() { count } else { 1 };
    bytes = bytes
        .checked_add(
            writers
                .checked_mul(IO_BUFFER_BYTES + std::mem::size_of::<BufWriter<File>>())
                .ok_or_else(overflow)?,
        )
        .ok_or_else(overflow)?;
    if count != 0 {
        // The plan rejects capacity above this existing bound after try_reserve_exact. Its
        // shared revision is already pinned/accounted by the job, not copied into this quote.
        let plan = std::mem::size_of::<OutputCapturePlan>()
            .checked_add(
                hex_engine::MAX_OUTPUT_CAPTURES
                    .checked_mul(std::mem::size_of::<OutputCaptureId>())
                    .ok_or_else(overflow)?,
            )
            .ok_or_else(overflow)?;
        let outputs = count
            .checked_mul(block + std::mem::size_of::<&mut [Frame]>())
            .ok_or_else(overflow)?;
        bytes = bytes
            .checked_add(plan)
            .and_then(|bytes| bytes.checked_add(outputs))
            .ok_or_else(overflow)?;
    }
    if stems.is_some() {
        let ordered = count
            .checked_mul(
                std::mem::size_of::<super::Stem>() + 4096 + std::mem::size_of::<OutputCaptureId>(),
            )
            .ok_or_else(overflow)?;
        // The encoded block budget above is reused for each stem.
        bytes = bytes.checked_add(ordered).ok_or_else(overflow)?;
    }
    Ok(bytes)
}

/// Resolve an optional range for the controlled renderer. Explicit spans remain
/// exact. Automatic ranges include declared finite residual tails on the
/// relevant signal paths and refuse unbounded or excessive drain. Main output is
/// latency-compensated (see [`render_prepared_with_policy`]), so its automatic
/// end is the causal Main end less Main latency, in timeline frames.
pub fn controlled_frame_count(
    project: &Project,
    start: u64,
    frames: Option<u64>,
    signal: &ExportSignal,
) -> Result<u64, ExportError> {
    let frames = match frames {
        Some(frames) => frames,
        None => {
            let end = match signal {
                ExportSignal::Main => project
                    .untrimmed_main_end_frame()
                    .map_err(|_| {
                        ExportError::Invalid(
                            "automatic main range is unbounded or invalid; choose an explicit range",
                        )
                    })?
                    .map(|end| end.saturating_sub(u64::from(project.main_latency_frames()))),
                ExportSignal::CapturedMix(ids) => {
                    if ids.len() > hex_engine::MAX_OUTPUT_CAPTURES {
                        return Err(ExportError::Invalid(
                            "invalid export output selection count",
                        ));
                    }
                    project.untrimmed_capture_end_frame(ids).map_err(|_| {
                        ExportError::Invalid(
                            "automatic capture range is unbounded or invalid; choose an explicit range",
                        )
                    })?
                }
                ExportSignal::Stems(stems) => {
                    if stems.len() > hex_engine::MAX_OUTPUT_CAPTURES {
                        return Err(ExportError::Invalid("invalid stem selection count"));
                    }
                    let mut ids = Vec::new();
                    ids.try_reserve_exact(stems.len())
                        .map_err(|_| ExportError::Invalid("could not reserve stem selection"))?;
                    ids.extend(stems.iter().map(|stem| stem.id));
                    project.untrimmed_capture_end_frame(&ids).map_err(|_| {
                        ExportError::Invalid(
                            "automatic stem range is unbounded or invalid; choose an explicit range",
                        )
                    })?
                }
            };
            end.unwrap_or(0).saturating_sub(start)
        }
    };
    if frames == 0 || start.checked_add(frames).is_none() {
        return Err(ExportError::Invalid("invalid export range"));
    }
    Ok(frames)
}

/// Complete description of a controlled prepared-revision export job.
pub struct ExportJobSpec<'a> {
    pub path: &'a Path,
    pub revision: Arc<PreparedRevision>,
    pub resources: Option<&'a PreparedDeviceResources>,
    pub start: u64,
    pub frames: u64,
    pub signal: ExportSignal,
    pub format: ExportFormat,
    pub control: &'a ExportControl,
    pub publication: &'a Publication,
}

/// Complete description of a linear Main export through caller-owned foreign processors.
pub struct ForeignLinearExportSpec<'a> {
    pub path: &'a Path,
    pub revision: Arc<PreparedRevision>,
    pub resources: Option<&'a PreparedDeviceResources>,
    pub start: u64,
    pub frames: u64,
    pub format: ExportFormat,
    pub control: &'a ExportControl,
    pub publication: &'a Publication,
}

/// Render a selected half-open frame range to a new dithered PCM16 WAV. Stateful history
/// is reconstructed from zero without Seek, in cancellable 256-frame blocks.
/// Preroll has the existing bounded engine work ceiling. Authored Arrangement
/// source loops render on the linear timeline without transport wrapping.
/// No live/Session history is captured.
pub fn render_prepared_controlled(
    path: &Path,
    revision: Arc<PreparedRevision>,
    start: u64,
    frames: u64,
    signal: ExportSignal,
    control: &ExportControl,
) -> Result<ExportReport, ExportError> {
    render_prepared_with_policy(ExportJobSpec {
        path,
        revision,
        resources: None,
        start,
        frames,
        signal,
        format: ExportFormat::Pcm16,
        control,
        publication: &Publication::CreateNew,
    })
}

/// Controlled range export under an explicit publication policy.
///
/// Main output is latency-compensated: file frame 0 is timeline
/// frame `start` and the file holds exactly `frames` frames. History is
/// reconstructed causally, then `frames + main latency` frames render and the
/// first `main latency` output frames are discarded, so compensation consumes
/// preroll rather than audio. Captured mixes and stems keep their causal
/// pre-Main capture timing. `format` selects the WAV sample format of the file
/// or of every stem file in a batch.
pub fn render_prepared_with_policy(spec: ExportJobSpec<'_>) -> Result<ExportReport, ExportError> {
    render_prepared_using(spec)
}

/// Causal range export through exact ordinary resource scopes. Preroll, selected outputs,
/// cancellation and publication match the resource-free controlled path. The caller owns and
/// admits the borrowed source set until this synchronous operation and runtime destruction end.
pub fn render_prepared_controlled_with_device_resources(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: &PreparedDeviceResources,
    start: u64,
    frames: u64,
    signal: ExportSignal,
    control: &ExportControl,
) -> Result<ExportReport, ExportError> {
    render_prepared_using(ExportJobSpec {
        path,
        revision,
        resources: Some(resources),
        start,
        frames,
        signal,
        format: ExportFormat::Pcm16,
        control,
        publication: &Publication::CreateNew,
    })
}

/// The same ordinary resource range with existing explicit overwrite consent semantics.
pub fn render_prepared_with_device_resources_and_policy<'a>(
    mut spec: ExportJobSpec<'a>,
    resources: &'a PreparedDeviceResources,
) -> Result<ExportReport, ExportError> {
    spec.resources = Some(resources);
    render_prepared_using(spec)
}

fn render_prepared_using(spec: ExportJobSpec<'_>) -> Result<ExportReport, ExportError> {
    render_prepared_using_pages(spec, None)
}

/// Linear export with caller-owned, independently verified offline pages.
/// Optional ordinary resources borrow the exact admitted compiler scope; page
/// preparation does not create or replace those leases. Preparation covers
/// preroll and every output/capture/stem chunk. A miss or
/// failed final verification refuses ordinary publication; no resident fallback.
pub fn render_prepared_with_pages_and_policy(
    spec: ExportJobSpec<'_>,
    pages: &mut dyn PreparedExportPages,
) -> Result<ExportReport, ExportError> {
    render_prepared_using_pages(spec, Some(pages))
}

/// Controlled range export with explicit post-processing [`ExportOptions`]
/// (optional deterministic windowed-sinc target sample-rate conversion and
/// explicit opt-in peak or LUFS loudness normalization) and optional verified
/// offline pages.
///
/// When `options` is identity (no rate change and normalization off), rendering
/// streams directly to the staged WAV or FLAC output with zero extra buffering.
/// When target SRC or normalization is requested, the compensated Arrangement
/// output is rendered in cancellable blocks, resampled via `hex-media` V2 SRC,
/// normalized with the exact applied gain reported on [`ExportReport`], and
/// published atomically under the same [`Publication`] policy.
pub fn render_prepared_with_options(
    spec: ExportJobSpec<'_>,
    options: ExportOptions,
    mut pages: Option<&mut dyn PreparedExportPages>,
) -> Result<ExportReport, ExportError> {
    options.validate()?;
    let source_rate = spec.revision.arrangement().sample_rate();
    if options.is_identity(source_rate) {
        return render_prepared_using_pages(spec, pages);
    }
    let ExportJobSpec {
        path,
        revision,
        resources,
        start,
        frames,
        signal,
        format,
        control,
        publication,
    } = spec;
    if matches!(signal, ExportSignal::Stems(_)) {
        return Err(ExportError::Invalid(
            "stem batch export does not support target sample-rate conversion or normalization",
        ));
    }
    preflight_destination(path, publication)?;
    let target_rate = options.target_sample_rate.unwrap_or(source_rate);
    let source_frame_count = usize::try_from(frames)
        .ok()
        .filter(|&count| count <= crate::post::MAX_POST_PROCESSED_FRAMES)
        .ok_or(ExportError::Invalid(
            "post-processed export frame count exceeds bounded limit",
        ))?;

    control.claim()?;
    let latency = main_compensation(&revision, &signal, frames);
    let rendered_frames = compensated_frames(start, frames, latency)?;

    let playback = prepare_runtime_with_pages(
        revision,
        resources,
        start,
        rendered_frames,
        control,
        pages.as_deref_mut(),
    )?;
    let (plan, buffers) = match signal {
        ExportSignal::Main => (None, Vec::new()),
        ExportSignal::CapturedMix(ids) => {
            if ids.is_empty() {
                return Err(ExportError::Invalid("export output selection is empty"));
            }
            let plan = playback
                .output_capture_plan(&ids, FRAMES_PER_BLOCK)
                .map_err(|_| ExportError::Invalid("invalid export output selection"))?;
            let mut buffers = Vec::new();
            buffers
                .try_reserve_exact(ids.len())
                .map_err(|_| ExportError::Invalid("could not reserve export capture buffers"))?;
            for _ in &ids {
                buffers.push([[0.0; 2]; FRAMES_PER_BLOCK]);
            }
            (Some(plan), buffers)
        }
        ExportSignal::Stems(_) => {
            return Err(ExportError::Invalid(
                "stem batch export does not support target sample-rate conversion or normalization",
            ));
        }
    };
    let mut renderer = ControlledRenderer {
        playback,
        sources: None,
        pages,
        plan,
        buffers,
        control: control.clone(),
    };
    discard_latency(&mut renderer, latency)?;

    let mut rendered = Vec::new();
    rendered
        .try_reserve_exact(source_frame_count)
        .map_err(|_| ExportError::Invalid("could not allocate post-processed export buffer"))?;
    let mut block = [[0.0_f32; 2]; FRAMES_PER_BLOCK];
    let mut remaining = source_frame_count;
    while remaining > 0 {
        let count = remaining.min(FRAMES_PER_BLOCK);
        renderer.render_block(&mut block[..count])?;
        for &[l, r] in &block[..count] {
            if !l.is_finite() || !r.is_finite() {
                return Err(ExportError::Invalid("render produced a non-finite sample"));
            }
        }
        rendered.extend_from_slice(&block[..count]);
        remaining -= count;
    }
    if let Some(pages) = renderer.pages.as_deref_mut() {
        pages.finish(control)?;
    }
    control.check()?;

    // Perform deterministic windowed-sinc target SRC if requested, polling live cancellation.
    let mut processed =
        crate::post::resample_export_frames(source_rate, target_rate, rendered, control)?;
    control.check()?;

    // Apply explicit opt-in peak or LUFS normalization and capture exact applied gain.
    let applied_gain = crate::post::apply_normalization(
        &mut processed,
        target_rate,
        options.normalization,
        control,
    )?;
    control.check()?;

    let output_frames = processed.len() as u64;
    let output_start = if target_rate == source_rate { start } else { 0 };
    publish_encoded_with_gain(
        path,
        target_rate,
        output_start,
        output_frames,
        publication,
        format,
        applied_gain,
        || {
            Ok(BufferedExportRenderer {
                frames: processed,
                offset: 0,
                control: control.clone(),
            })
        },
    )
}

struct BufferedExportRenderer {
    frames: Vec<Frame>,
    offset: usize,
    control: ExportControl,
}

impl BlockRenderer for BufferedExportRenderer {
    fn render_block(&mut self, out: &mut [Frame]) -> Result<(), ExportError> {
        self.control.check()?;
        let end = self
            .offset
            .checked_add(out.len())
            .filter(|&end| end <= self.frames.len())
            .ok_or(ExportError::Invalid("buffered export underrun"))?;
        out.copy_from_slice(&self.frames[self.offset..end]);
        self.offset = end;
        Ok(())
    }

    fn before_publication(&mut self) -> Result<(), ExportError> {
        self.control.begin_publication()
    }
}

/// Render Main, CapturedMix (solo-source / chosen-track), or Stems through
/// independently owned offline foreign processors and optional verified pages.
/// Plugins requiring real-time rendering refuse before touching destination files.
pub fn render_prepared_with_foreign(
    spec: ExportJobSpec<'_>,
    pages: Option<&mut dyn PreparedExportPages>,
    foreign: &mut dyn crate::ForeignBounceRenderer,
) -> Result<ExportReport, ExportError> {
    if foreign.requires_realtime_fallback() {
        return Err(ExportError::Invalid(
            "offline vst3 plugin requires real-time render fallback",
        ));
    }
    let ExportJobSpec {
        path,
        revision,
        resources,
        start,
        frames,
        signal,
        format,
        control,
        publication,
    } = spec;
    match signal {
        ExportSignal::Main => render_linear_main_with_foreign(
            ForeignLinearExportSpec {
                path,
                revision,
                resources,
                start,
                frames,
                format,
                control,
                publication,
            },
            pages,
            foreign,
        ),
        ExportSignal::Stems(stems) => {
            if !matches!(publication, Publication::CreateNew) {
                return Err(ExportError::Invalid("stem batches require a new directory"));
            }
            super::stems::render_stems_using_pages_and_foreign(
                super::stems::StemBatchSpec {
                    path,
                    revision,
                    resources,
                    start,
                    frames,
                    stems: &stems,
                    format,
                    control,
                },
                pages,
                foreign,
            )
        }
        ExportSignal::CapturedMix(ids) => {
            if ids.is_empty() {
                return Err(ExportError::Invalid("export output selection is empty"));
            }
            let block_frames = foreign.max_block_frames().min(FRAMES_PER_BLOCK);
            if block_frames == 0 || frames == 0 {
                return Err(ExportError::Invalid(
                    "invalid foreign export block or range",
                ));
            }
            control.claim()?;
            let sample_rate = revision.arrangement().sample_rate();
            let rendered_frames = compensated_frames(start, frames, 0)?;
            publish_encoded(
                path,
                sample_rate,
                start,
                frames,
                publication,
                format,
                || {
                    let mut inputs = crate::bounce::RenderInputs {
                        pages,
                        foreign: Some(foreign),
                    };
                    let playback = prepare_runtime_with_inputs(
                        RuntimePrepSpec {
                            revision,
                            resources,
                            start,
                            frames: rendered_frames,
                            control,
                        },
                        foreign_linear_runtime,
                        || {},
                        &mut inputs,
                        block_frames,
                    )?;
                    let plan = playback
                        .output_capture_plan(&ids, block_frames)
                        .map_err(|_| ExportError::Invalid("invalid export output selection"))?;
                    let mut buffers = Vec::new();
                    buffers.try_reserve_exact(ids.len()).map_err(|_| {
                        ExportError::Invalid("could not reserve export capture buffers")
                    })?;
                    for _ in &ids {
                        buffers.push([[0.0; 2]; FRAMES_PER_BLOCK]);
                    }
                    Ok(ForeignCapturedRenderer {
                        playback,
                        inputs,
                        plan,
                        buffers,
                        block_frames,
                        control: control.clone(),
                    })
                },
            )
        }
    }
}

/// Render linear Main through independently owned offline processors and,
/// optionally, verified pages. Both owners finish before publication. Their
/// custody remains with the caller and failures abort the staged export.
pub fn render_linear_main_with_foreign(
    spec: ForeignLinearExportSpec<'_>,
    pages: Option<&mut dyn PreparedExportPages>,
    foreign: &mut dyn crate::ForeignBounceRenderer,
) -> Result<ExportReport, ExportError> {
    if foreign.requires_realtime_fallback() {
        return Err(ExportError::Invalid(
            "offline vst3 plugin requires real-time render fallback",
        ));
    }
    let ForeignLinearExportSpec {
        path,
        revision,
        resources,
        start,
        frames,
        format,
        control,
        publication,
    } = spec;
    let block_frames = foreign.max_block_frames().min(FRAMES_PER_BLOCK);
    if block_frames == 0 || frames == 0 {
        return Err(ExportError::Invalid(
            "invalid foreign export block or range",
        ));
    }
    control.claim()?;
    let sample_rate = revision.arrangement().sample_rate();
    let latency = main_compensation(&revision, &ExportSignal::Main, frames);
    let rendered_frames = compensated_frames(start, frames, latency)?;
    publish_encoded(
        path,
        sample_rate,
        start,
        frames,
        publication,
        format,
        || {
            let mut inputs = crate::bounce::RenderInputs {
                pages,
                foreign: Some(foreign),
            };
            let playback = prepare_runtime_with_inputs(
                RuntimePrepSpec {
                    revision,
                    resources,
                    start,
                    frames: rendered_frames,
                    control,
                },
                foreign_linear_runtime,
                || {},
                &mut inputs,
                block_frames,
            )?;
            let mut renderer = ForeignLinearRenderer {
                playback,
                inputs,
                block_frames,
                control: control.clone(),
            };
            discard_latency(&mut renderer, latency)?;
            Ok(renderer)
        },
    )
}

pub(super) fn foreign_linear_runtime(
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    _frames: u64,
) -> Result<SessionPlayback, hex_engine::SessionPlaybackError> {
    SessionPlayback::new_linear_arrangement_export_with_foreign(revision, resources)
}

struct ForeignCapturedRenderer<'p, 'pages, 'f> {
    playback: SessionPlayback,
    inputs: crate::bounce::RenderInputs<'p, 'pages, 'f>,
    plan: OutputCapturePlan,
    buffers: Vec<[Frame; FRAMES_PER_BLOCK]>,
    block_frames: usize,
    control: ExportControl,
}

impl BlockRenderer for ForeignCapturedRenderer<'_, '_, '_> {
    fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
        for chunk in frames.chunks_mut(self.block_frames) {
            self.control.check()?;
            let mut slices: Vec<&mut [Frame]> = self
                .buffers
                .iter_mut()
                .map(|buffer| &mut buffer[..chunk.len()])
                .collect();
            let mut capture = OutputCapture::new(&self.plan, &mut slices);
            if let Some(pages) = self.inputs.pages.as_deref_mut() {
                pages.prepare_chunk(&self.playback, chunk.len(), &self.control)?;
                let mut view = crate::pages::PageView(pages);
                let foreign = self
                    .inputs
                    .foreign
                    .as_deref_mut()
                    .ok_or(ExportError::Invalid("offline vst3 renderer unavailable"))?;
                foreign.render_block_with_capture(
                    &mut self.playback,
                    chunk,
                    Some(&mut view),
                    &mut capture,
                    &self.control,
                )?;
                pages.complete_chunk()?;
            } else {
                let foreign = self
                    .inputs
                    .foreign
                    .as_deref_mut()
                    .ok_or(ExportError::Invalid("offline vst3 renderer unavailable"))?;
                foreign.render_block_with_capture(
                    &mut self.playback,
                    chunk,
                    None,
                    &mut capture,
                    &self.control,
                )?;
            }
            chunk.fill([0.0; 2]);
            for index in 0..self.plan.ids().len() {
                let output = capture
                    .output(index)
                    .ok_or(ExportError::Invalid("export capture was not committed"))?;
                for (destination, source) in chunk.iter_mut().zip(output) {
                    destination[0] += source[0];
                    destination[1] += source[1];
                }
            }
            self.control.advance(chunk.len());
        }
        self.control.check()
    }

    fn before_publication(&mut self) -> Result<(), ExportError> {
        if let Some(pages) = self.inputs.pages.as_deref_mut() {
            pages.finish(&self.control)?;
        }
        if let Some(foreign) = self.inputs.foreign.as_deref_mut() {
            foreign.finish(&self.control)?;
        }
        self.control.begin_publication()
    }
}

struct ForeignLinearRenderer<'p, 'pages, 'f> {
    playback: SessionPlayback,
    inputs: crate::bounce::RenderInputs<'p, 'pages, 'f>,
    block_frames: usize,
    control: ExportControl,
}

impl BlockRenderer for ForeignLinearRenderer<'_, '_, '_> {
    fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
        for chunk in frames.chunks_mut(self.block_frames) {
            self.control.check()?;
            self.inputs
                .render(&mut self.playback, chunk, &self.control)?;
            self.control.advance(chunk.len());
        }
        self.control.check()
    }

    fn before_publication(&mut self) -> Result<(), ExportError> {
        if let Some(pages) = self.inputs.pages.as_deref_mut() {
            pages.finish(&self.control)?;
        }
        if let Some(foreign) = self.inputs.foreign.as_deref_mut() {
            foreign.finish(&self.control)?;
        }
        self.control.begin_publication()
    }
}

fn render_prepared_using_pages(
    spec: ExportJobSpec<'_>,
    mut pages: Option<&mut dyn PreparedExportPages>,
) -> Result<ExportReport, ExportError> {
    let ExportJobSpec {
        path,
        revision,
        resources,
        start,
        frames,
        signal,
        format,
        control,
        publication,
    } = spec;
    if let ExportSignal::Stems(stems) = &signal {
        if !matches!(publication, Publication::CreateNew) {
            return Err(ExportError::Invalid("stem batches require a new directory"));
        }
        return super::stems::render_stems_using_pages(
            super::stems::StemBatchSpec {
                path,
                revision,
                resources,
                start,
                frames,
                stems,
                format,
                control,
            },
            pages,
        );
    }
    control.claim()?;
    let sample_rate = revision.arrangement().sample_rate();
    let latency = main_compensation(&revision, &signal, frames);
    let rendered_frames = compensated_frames(start, frames, latency)?;
    let prepare_renderer = || {
        let playback = prepare_runtime_with_pages(
            revision,
            resources,
            start,
            rendered_frames,
            control,
            pages.as_deref_mut(),
        )?;
        let (plan, buffers) = match signal {
            ExportSignal::Main => (None, Vec::new()),
            ExportSignal::CapturedMix(ids) => {
                if ids.is_empty() {
                    return Err(ExportError::Invalid("export output selection is empty"));
                }
                let plan = playback
                    .output_capture_plan(&ids, FRAMES_PER_BLOCK)
                    .map_err(|_| ExportError::Invalid("invalid export output selection"))?;
                let mut buffers = Vec::new();
                buffers.try_reserve_exact(ids.len()).map_err(|_| {
                    ExportError::Invalid("could not reserve export capture buffers")
                })?;
                for _ in &ids {
                    buffers.push([[0.0; 2]; FRAMES_PER_BLOCK]);
                }
                (Some(plan), buffers)
            }
            ExportSignal::Stems(_) => {
                return Err(ExportError::Invalid("stem batches require batch rendering"));
            }
        };
        let mut renderer = Box::new(ControlledRenderer {
            playback,
            sources: None,
            pages,
            plan,
            buffers,
            control: control.clone(),
        });
        discard_latency(&mut *renderer, latency)?;
        Ok(renderer)
    };
    publish_encoded(
        path,
        sample_rate,
        start,
        frames,
        publication,
        format,
        prepare_renderer,
    )
}

/// Main latency to discard after causal preroll; captures keep causal timing.
fn main_compensation(revision: &PreparedRevision, signal: &ExportSignal, frames: u64) -> u32 {
    if frames == 0 || !matches!(signal, ExportSignal::Main) {
        0
    } else {
        revision.arrangement().main_latency_frames()
    }
}

/// Frames rendered after preroll: the published span plus discarded latency.
fn compensated_frames(start: u64, frames: u64, latency: u32) -> Result<u64, ExportError> {
    let rendered = frames
        .checked_add(u64::from(latency))
        .ok_or(ExportError::Invalid("compensated render range overflows"))?;
    start
        .checked_add(rendered)
        .ok_or(ExportError::Invalid("compensated render range overflows"))?;
    Ok(rendered)
}

/// Explicit private-control source capability. Same publication, cancellation,
/// block renderer and SessionPlayback implementation as ordinary export.
pub fn render_control_sampler_with_policy(
    spec: ExportJobSpec<'_>,
    sources: hex_engine::native_device_resources::ControlSamplerSources,
) -> Result<ExportReport, ExportError> {
    let ExportJobSpec {
        path,
        revision,
        resources: _,
        start,
        frames,
        signal,
        format,
        control,
        publication,
    } = spec;
    if start != 0 || !matches!(signal, ExportSignal::Main) {
        return Err(ExportError::Invalid(
            "sampler supports frame-zero main export only",
        ));
    }
    control.claim()?;
    let latency = main_compensation(&revision, &signal, frames);
    control
        .total
        .store(compensated_frames(0, frames, latency)?, Ordering::Release);
    let sample_rate = revision.arrangement().sample_rate();
    publish_encoded(path, sample_rate, 0, frames, publication, format, || {
        control.check()?;
        let mut playback = SessionPlayback::new_control_sampler_export(revision, &sources)
            .map_err(ExportError::SessionRender)?;
        playback
            .apply(SessionPlaybackCommand::Play)
            .map_err(ExportError::SessionRender)?;
        let mut renderer = Box::new(ControlledRenderer {
            playback,
            sources: Some(sources),
            pages: None,
            plan: None,
            buffers: Vec::new(),
            control: control.clone(),
        });
        discard_latency(&mut *renderer, latency)?;
        Ok(renderer)
    })
}

pub(super) fn prepare_runtime(
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    control: &ExportControl,
) -> Result<SessionPlayback, ExportError> {
    prepare_runtime_observed(revision, resources, start, frames, control, || {})
}

pub(super) struct RuntimePrepSpec<'a> {
    pub(super) revision: Arc<PreparedRevision>,
    pub(super) resources: Option<&'a PreparedDeviceResources>,
    pub(super) start: u64,
    pub(super) frames: u64,
    pub(super) control: &'a ExportControl,
}

pub(super) fn prepare_runtime_with_foreign<'provider>(
    spec: RuntimePrepSpec<'_>,
    pages: Option<&mut (dyn PreparedExportPages + 'provider)>,
    foreign: &mut dyn crate::ForeignBounceRenderer,
    block_frames: usize,
) -> Result<SessionPlayback, ExportError> {
    let mut inputs = crate::bounce::RenderInputs {
        pages,
        foreign: Some(foreign),
    };
    prepare_runtime_with_inputs(
        spec,
        foreign_linear_runtime,
        || {},
        &mut inputs,
        block_frames,
    )
}

pub(super) fn prepare_runtime_with_pages<'provider>(
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    control: &ExportControl,
    pages: Option<&mut (dyn PreparedExportPages + 'provider)>,
) -> Result<SessionPlayback, ExportError> {
    let constructor: RuntimeConstructor = if resources.is_some() {
        resource_linear_runtime
    } else {
        legacy_linear_runtime
    };
    prepare_runtime_using_pages(
        RuntimePrepSpec {
            revision,
            resources,
            start,
            frames,
            control,
        },
        constructor,
        || {},
        pages,
    )
}

fn prepare_runtime_observed(
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    control: &ExportControl,
    after_block: impl FnMut(),
) -> Result<SessionPlayback, ExportError> {
    let constructor: RuntimeConstructor = if resources.is_some() {
        resource_linear_runtime
    } else {
        legacy_linear_runtime
    };
    prepare_runtime_using(
        RuntimePrepSpec {
            revision,
            resources,
            start,
            frames,
            control,
        },
        constructor,
        after_block,
    )
}

pub(super) fn prepare_bounce_runtime<'provider>(
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    control: &ExportControl,
    pages: Option<&mut (dyn PreparedExportPages + 'provider)>,
) -> Result<SessionPlayback, ExportError> {
    prepare_bounce_runtime_with_inputs(
        revision,
        resources,
        start,
        frames,
        control,
        &mut crate::bounce::RenderInputs {
            pages,
            foreign: None,
        },
        FRAMES_PER_BLOCK,
    )
}

pub(super) fn prepare_bounce_runtime_with_inputs(
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    control: &ExportControl,
    inputs: &mut crate::bounce::RenderInputs<'_, '_, '_>,
    block_frames: usize,
) -> Result<SessionPlayback, ExportError> {
    let constructor: RuntimeConstructor = if inputs.foreign.is_some() {
        SessionPlayback::new_finite_arrangement_bounce_with_foreign
    } else {
        SessionPlayback::new_finite_arrangement_bounce
    };
    prepare_runtime_with_inputs(
        RuntimePrepSpec {
            revision,
            resources,
            start,
            frames,
            control,
        },
        constructor,
        || {},
        inputs,
        block_frames,
    )
}

type RuntimeConstructor = fn(
    Arc<PreparedRevision>,
    Option<&PreparedDeviceResources>,
    u64,
) -> Result<SessionPlayback, hex_engine::SessionPlaybackError>;

fn prepare_runtime_using(
    spec: RuntimePrepSpec<'_>,
    constructor: RuntimeConstructor,
    after_block: impl FnMut(),
) -> Result<SessionPlayback, ExportError> {
    prepare_runtime_using_pages(spec, constructor, after_block, None)
}

fn prepare_runtime_using_pages<'provider>(
    spec: RuntimePrepSpec<'_>,
    constructor: RuntimeConstructor,
    after_block: impl FnMut(),
    pages: Option<&mut (dyn PreparedExportPages + 'provider)>,
) -> Result<SessionPlayback, ExportError> {
    prepare_runtime_with_inputs(
        spec,
        constructor,
        after_block,
        &mut crate::bounce::RenderInputs {
            pages,
            foreign: None,
        },
        FRAMES_PER_BLOCK,
    )
}

fn prepare_runtime_with_inputs(
    spec: RuntimePrepSpec<'_>,
    constructor: RuntimeConstructor,
    mut after_block: impl FnMut(),
    inputs: &mut crate::bounce::RenderInputs<'_, '_, '_>,
    block_frames: usize,
) -> Result<SessionPlayback, ExportError> {
    let RuntimePrepSpec {
        revision,
        resources,
        start,
        frames,
        control,
    } = spec;
    control.check()?;
    let replay = start != 0
        && if inputs.foreign.is_some() {
            revision.arrangement_bounce_requires_preroll()
        } else {
            revision.arrangement_seek_requires_preroll()
        };
    if replay && start > hex_engine::MAX_ARRANGEMENT_PREROLL_FRAMES {
        return Err(ExportError::Invalid(
            "export preroll exceeds its frame budget",
        ));
    }
    control.total.store(
        (if replay { start } else { 0 })
            .checked_add(frames)
            .ok_or(ExportError::Invalid("frame range overflows"))?,
        Ordering::Release,
    );
    // Construct exactly once with the selected policy; source scope validation
    // stays in the engine, and factories mint leases only on this control thread.
    let mut playback =
        constructor(revision, resources, frames).map_err(ExportError::SessionRender)?;
    if !replay && start != 0 {
        playback
            .apply(SessionPlaybackCommand::Seek(start))
            .map_err(ExportError::SessionRender)?;
    }
    playback
        .apply(SessionPlaybackCommand::Play)
        .map_err(ExportError::SessionRender)?;
    let mut remaining = if replay { start } else { 0 };
    let mut block = [[0.0; 2]; FRAMES_PER_BLOCK];
    while remaining > 0 {
        control.check()?;
        let count = remaining.min(block_frames as u64) as usize;
        inputs.render(&mut playback, &mut block[..count], control)?;
        remaining -= count as u64;
        control.advance(count);
        after_block();
    }
    control.check()?;
    Ok(playback)
}

fn legacy_linear_runtime(
    revision: Arc<PreparedRevision>,
    _resources: Option<&PreparedDeviceResources>,
    _frames: u64,
) -> Result<SessionPlayback, hex_engine::SessionPlaybackError> {
    SessionPlayback::new_linear_arrangement_export(revision)
}
fn resource_linear_runtime(
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    _frames: u64,
) -> Result<SessionPlayback, hex_engine::SessionPlaybackError> {
    let resources = resources.ok_or(
        hex_engine::SessionPlaybackError::MutableInstrumentDiagnostic(
            "export device resources unavailable",
        ),
    )?;
    SessionPlayback::new_linear_arrangement_export_with_device_resources(revision, resources)
}

struct ControlledRenderer<'a> {
    playback: SessionPlayback,
    sources: Option<hex_engine::native_device_resources::ControlSamplerSources>,
    pages: Option<&'a mut dyn PreparedExportPages>,
    plan: Option<OutputCapturePlan>,
    buffers: Vec<[Frame; FRAMES_PER_BLOCK]>,
    control: ExportControl,
}

impl BlockRenderer for ControlledRenderer<'_> {
    fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
        self.control.check()?;
        if let Some(pages) = self.pages.as_deref_mut() {
            pages.prepare_chunk(&self.playback, frames.len(), &self.control)?;
        }
        if let Some(plan) = &self.plan {
            let mut slices: Vec<&mut [Frame]> = self
                .buffers
                .iter_mut()
                .map(|buffer| &mut buffer[..frames.len()])
                .collect();
            let mut capture = OutputCapture::new(plan, &mut slices);
            if let Some(pages) = self.pages.as_deref_mut() {
                self.playback
                    .render_with_pages_and_capture(
                        frames,
                        &mut crate::pages::PageView(pages),
                        &mut capture,
                    )
                    .map_err(ExportError::SessionRender)?;
            } else {
                self.playback
                    .render_with_capture(frames, &mut capture)
                    .map_err(ExportError::SessionRender)?;
            }
            frames.fill([0.0; 2]);
            for index in 0..plan.ids().len() {
                let output = capture
                    .output(index)
                    .ok_or(ExportError::Invalid("export capture was not committed"))?;
                for (destination, source) in frames.iter_mut().zip(output) {
                    destination[0] += source[0];
                    destination[1] += source[1];
                }
            }
        } else if let Some(sources) = self.sources.as_ref() {
            self.playback
                .render_control_sampler_export(frames, sources)
                .map_err(ExportError::SessionRender)?;
        } else if let Some(pages) = self.pages.as_deref_mut() {
            self.playback
                .render_with_pages(frames, &mut crate::pages::PageView(pages))
                .map_err(ExportError::SessionRender)?;
        } else {
            self.playback
                .render(frames)
                .map_err(ExportError::SessionRender)?;
        }
        if let Some(pages) = self.pages.as_deref_mut() {
            pages.complete_chunk()?;
        }
        self.control.advance(frames.len());
        self.control.check()
    }

    fn before_publication(&mut self) -> Result<(), ExportError> {
        if let Some(pages) = self.pages.as_deref_mut() {
            pages.finish(&self.control)?;
        }
        self.control.begin_publication()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    use hex_project::{Command, Created, Editor};

    #[test]
    fn bounce_progress_includes_frame_work_and_never_resets_between_outputs() {
        let job = ExportControl::default();
        let first = job.for_bounce_item(0, 2).unwrap();
        first.total.store(400, Ordering::Relaxed);
        first.advance(100);
        assert!((job.bounce_fraction() - 0.125).abs() < 0.0002);
        first.advance(300);
        assert!(job.bounce_fraction() < 0.5);
        first.finish_bounce_item();
        assert_eq!(job.bounce_fraction(), 0.5);

        let second = job.for_bounce_item(1, 2).unwrap();
        second.total.store(800, Ordering::Relaxed);
        assert_eq!(job.bounce_fraction(), 0.5);
        second.advance(400);
        assert!((job.bounce_fraction() - 0.75).abs() < 0.0002);
        assert!(job.cancel());
        assert!(second.is_cancelled());
        assert!(job.bounce_fraction() < 1.0);
    }

    #[test]
    fn control_storage_quote_counts_owned_capacity_and_rejects_unbounded_selection() {
        let mut editor = Editor::new(hex_project::Project::new(48000).unwrap(), 1).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let mut selected = Vec::with_capacity(8);
        selected.push(OutputCaptureId::Track(track));
        let narrow = ExportSignal::CapturedMix(vec![OutputCaptureId::Track(track)]);
        let wide = ExportSignal::CapturedMix(selected);
        assert!(
            control_storage_allowance(Some(&wide)).unwrap()
                > control_storage_allowance(Some(&narrow)).unwrap()
        );
        let mut name = String::with_capacity(8192);
        name.push('a');
        let mut owned = ExportSignal::Stems(vec![super::super::Stem {
            id: OutputCaptureId::Track(track),
            name,
        }]);
        let large = control_storage_allowance(Some(&owned)).unwrap();
        if let ExportSignal::Stems(stems) = &mut owned {
            stems[0].name.shrink_to_fit();
        }
        assert!(large > control_storage_allowance(Some(&owned)).unwrap());
        assert!(control_storage_allowance(Some(&ExportSignal::CapturedMix(Vec::new()))).is_err());
        assert!(
            control_storage_allowance(Some(&ExportSignal::CapturedMix(vec![
                OutputCaptureId::Track(
                    track
                );
                hex_engine::MAX_OUTPUT_CAPTURES
                    + 1
            ])))
            .is_err()
        );
        assert!(
            control_storage_allowance(Some(&ExportSignal::Stems(vec![super::super::Stem {
                id: OutputCaptureId::Track(track),
                name: "x".repeat(4097),
            }])))
            .is_err()
        );
    }

    #[test]
    fn preroll_progress_cancel_and_budget_refusal_never_return_partial_history() {
        let mut editor = Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "stateful".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let state = hex_engine::patch_native_device_parameter(
            &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                .unwrap(),
            ParameterId::from_static(11),
            ParameterValue::Bool(true),
        )
        .unwrap();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        assert!(revision.arrangement_seek_requires_preroll());
        let control = ExportControl::default();
        let result =
            prepare_runtime_observed(Arc::clone(&revision), None, 513, 100, &control, || {
                control.cancel();
            });
        assert!(matches!(result, Err(ExportError::Cancelled)));
        assert_eq!(
            control.progress(),
            ExportProgress {
                processed_frames: 256,
                total_frames: 613
            }
        );
        assert!(matches!(
            prepare_runtime(
                revision,
                None,
                hex_engine::MAX_ARRANGEMENT_PREROLL_FRAMES + 1,
                1,
                &ExportControl::default()
            ),
            Err(ExportError::Invalid(
                "export preroll exceeds its frame budget"
            ))
        ));
    }

    #[test]
    fn render_prepared_with_options_preflights_collision_before_rendering() {
        let dir = std::env::temp_dir().join(format!(
            "hex-export-preflight-collision-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("already-here.wav");
        std::fs::write(&existing, b"existing-user-audio").unwrap();

        let project = hex_project::Project::new(48_000).unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(
                &project,
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let control = ExportControl::default();
        let err = render_prepared_with_options(
            ExportJobSpec {
                path: &existing,
                revision,
                resources: None,
                start: 0,
                frames: 48_000,
                signal: ExportSignal::Main,
                format: ExportFormat::Pcm16,
                control: &control,
                publication: &Publication::CreateNew,
            },
            ExportOptions {
                target_sample_rate: Some(44_100),
                normalization: ExportNormalization::Peak { target_dbfs: -1.0 },
            },
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ExportError::Io { ref source, .. } if source.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(control.progress().processed_frames, 0);
        assert_eq!(std::fs::read(&existing).unwrap(), b"existing-user-audio");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
