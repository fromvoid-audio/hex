//! Control-worker rendering through one canonical linear Arrangement owner.
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

    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::Acquire) == 1
    }

    pub fn progress(&self) -> ExportProgress {
        ExportProgress {
            processed_frames: self.processed.load(Ordering::Acquire),
            total_frames: self.total.load(Ordering::Acquire),
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

    pub(super) fn advance(&self, count: usize) {
        self.processed.fetch_add(count as u64, Ordering::Release);
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

/// Render a selected half-open frame range to a new PCM16 WAV. Stateful history
/// is reconstructed from zero without Seek, in cancellable 256-frame blocks.
/// Preroll has the existing bounded engine work ceiling; transport loop metadata
/// is ignored by the canonical linear export owner. Source-loop restrictions
/// remain the engine's explicit refusals. No live/Session history is captured.
pub fn render_prepared_controlled(
    path: &Path,
    revision: Arc<PreparedRevision>,
    start: u64,
    frames: u64,
    signal: ExportSignal,
    control: &ExportControl,
) -> Result<ExportReport, ExportError> {
    render_prepared_with_policy(
        path,
        revision,
        start,
        frames,
        signal,
        control,
        &Publication::CreateNew,
    )
}

pub fn render_prepared_with_policy(
    path: &Path,
    revision: Arc<PreparedRevision>,
    start: u64,
    frames: u64,
    signal: ExportSignal,
    control: &ExportControl,
    publication: &Publication,
) -> Result<ExportReport, ExportError> {
    if let ExportSignal::Stems(stems) = &signal {
        if !matches!(publication, Publication::CreateNew) {
            return Err(ExportError::Invalid("stem batches require a new directory"));
        }
        return super::render_stems_new(path, revision, start, frames, stems, control);
    }
    control.claim()?;
    let sample_rate = revision.arrangement().sample_rate();
    publish(path, sample_rate, start, frames, publication, || {
        let playback = prepare_runtime(revision, start, frames, control)?;
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
        Ok(ControlledRenderer {
            playback,
            plan,
            buffers,
            control: control.clone(),
        })
    })
}

pub(super) fn prepare_runtime(
    revision: Arc<PreparedRevision>,
    start: u64,
    frames: u64,
    control: &ExportControl,
) -> Result<SessionPlayback, ExportError> {
    prepare_runtime_observed(revision, start, frames, control, || {})
}

fn prepare_runtime_observed(
    revision: Arc<PreparedRevision>,
    start: u64,
    frames: u64,
    control: &ExportControl,
    mut after_block: impl FnMut(),
) -> Result<SessionPlayback, ExportError> {
    control.check()?;
    let replay = start != 0 && revision.arrangement_seek_requires_preroll();
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
    let mut playback = SessionPlayback::new_linear_arrangement_export(revision)
        .map_err(ExportError::SessionRender)?;
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
        let count = remaining.min(FRAMES_PER_BLOCK as u64) as usize;
        playback
            .render(&mut block[..count])
            .map_err(ExportError::SessionRender)?;
        remaining -= count as u64;
        control.advance(count);
        after_block();
    }
    control.check()?;
    Ok(playback)
}

struct ControlledRenderer {
    playback: SessionPlayback,
    plan: Option<OutputCapturePlan>,
    buffers: Vec<[Frame; FRAMES_PER_BLOCK]>,
    control: ExportControl,
}

impl BlockRenderer for ControlledRenderer {
    fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
        self.control.check()?;
        if let Some(plan) = &self.plan {
            let mut slices: Vec<&mut [Frame]> = self
                .buffers
                .iter_mut()
                .map(|buffer| &mut buffer[..frames.len()])
                .collect();
            let mut capture = OutputCapture::new(plan, &mut slices);
            self.playback
                .render_with_capture(frames, &mut capture)
                .map_err(ExportError::SessionRender)?;
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
        } else {
            self.playback
                .render(frames)
                .map_err(ExportError::SessionRender)?;
        }
        self.control.advance(frames.len());
        self.control.check()
    }

    fn before_publication(&self) -> Result<(), ExportError> {
        self.control.begin_publication()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    use hex_project::{Command, Created, Editor};

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
                hex_engine::SharedSineTable::new_on_control(),
                None,
            )
            .unwrap(),
        );
        assert!(revision.arrangement_seek_requires_preroll());
        let control = ExportControl::default();
        let result = prepare_runtime_observed(Arc::clone(&revision), 513, 100, &control, || {
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
                hex_engine::MAX_ARRANGEMENT_PREROLL_FRAMES + 1,
                1,
                &ExportControl::default()
            ),
            Err(ExportError::Invalid(
                "export preroll exceeds its frame budget"
            ))
        ));
    }
}
