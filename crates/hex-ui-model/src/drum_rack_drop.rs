//! Pure GPUI-free Drum Rack pad drop target validation and delayed-ACK display
//! handoff state machine (`#3476`).
//!
//! Captures stable `(project_generation, revision, source_id, device_id, pad_id)`
//! identities at drag/drop time, validates accepted sample-file and native
//! instrument drop payloads with lowercase diagnostics, and retains an immutable
//! final drop preview across intermediate repaints via `pending_edit::Receipt`
//! until the exact synchronous terminal acknowledgement or project replacement.

use crate::pending_edit::{Disposition, Receipt};
use hex_project::SourceId;
use std::{num::NonZeroU64, path::PathBuf};

/// Supported lowercase audio file extensions accepted for Drum Rack pad sample drops.
pub const SUPPORTED_PAD_SAMPLE_EXTENSIONS: &[&str] = &["wav", "aif", "aiff", "flac", "ogg", "mp3"];

/// Reserved rack type IDs refused when dropped onto a Drum Rack pad (no arbitrary nesting).
const REFUSED_RACK_TYPE_IDS: &[&str] = &["hex.rack.drum", "hex.rack.instrument", "hex.rack.midi"];

/// Stable project & pad identities captured at drag start / drop release.
/// Carries metadata only — never grants ownership or mutates project state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrumRackPadDropTarget {
    pub project_generation: u64,
    pub revision: u64,
    pub source_id: Option<SourceId>,
    pub device_id: NonZeroU64,
    pub pad_id: NonZeroU64,
}

/// Typed drag payload offered to a Drum Rack pad drop target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrumRackPadDropPayload {
    NativeInstrument {
        type_id: String,
        display_name: String,
    },
    SampleFile {
        path: PathBuf,
    },
}

/// Visual drop-target evaluation shown while hovering over a Drum Rack pad.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PadDropTargetFeedback {
    Accepted {
        pad_id: NonZeroU64,
        preview_label: String,
    },
    Refused {
        pad_id: NonZeroU64,
        reason: &'static str,
    },
}

/// Evaluate whether `payload` is a valid drop onto `target` pad.
pub fn evaluate_pad_drop_target(
    target: DrumRackPadDropTarget,
    payload: &DrumRackPadDropPayload,
) -> PadDropTargetFeedback {
    match payload {
        DrumRackPadDropPayload::NativeInstrument {
            type_id,
            display_name,
        } => {
            let trimmed = type_id.trim();
            if trimmed.is_empty() {
                return PadDropTargetFeedback::Refused {
                    pad_id: target.pad_id,
                    reason: "unsupported instrument drop",
                };
            }
            if REFUSED_RACK_TYPE_IDS.contains(&trimmed) {
                return PadDropTargetFeedback::Refused {
                    pad_id: target.pad_id,
                    reason: "nested racks cannot be dropped onto a drum rack pad",
                };
            }
            let label = if display_name.trim().is_empty() {
                trimmed.to_ascii_lowercase()
            } else {
                display_name.trim().to_ascii_lowercase()
            };
            PadDropTargetFeedback::Accepted {
                pad_id: target.pad_id,
                preview_label: label,
            }
        }
        DrumRackPadDropPayload::SampleFile { path } => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            let Some(ext) = ext else {
                return PadDropTargetFeedback::Refused {
                    pad_id: target.pad_id,
                    reason: "unsupported sample file format",
                };
            };
            if !SUPPORTED_PAD_SAMPLE_EXTENSIONS.contains(&ext.as_str()) {
                return PadDropTargetFeedback::Refused {
                    pad_id: target.pad_id,
                    reason: "unsupported sample file format",
                };
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "sample".to_owned());
            PadDropTargetFeedback::Accepted {
                pad_id: target.pad_id,
                preview_label: stem,
            }
        }
    }
}

/// Immutable frozen preview retained after pointer release until exact worker
/// terminal acknowledgement or project replacement (`pending_edit::Receipt`).
#[derive(Debug)]
pub struct PendingPadDropDisplay {
    pub target: DrumRackPadDropTarget,
    pub preview_label: String,
    pub payload: DrumRackPadDropPayload,
    receipt: Receipt,
}

impl PendingPadDropDisplay {
    /// Capture an accepted drop release prior to session submission.
    pub fn from_release(
        target: DrumRackPadDropTarget,
        payload: DrumRackPadDropPayload,
    ) -> Result<Self, &'static str> {
        match evaluate_pad_drop_target(target, &payload) {
            PadDropTargetFeedback::Accepted { preview_label, .. } => Ok(Self {
                target,
                preview_label,
                payload,
                receipt: Receipt::new(target.project_generation),
            }),
            PadDropTargetFeedback::Refused { reason, .. } => Err(reason),
        }
    }

    pub fn is_queued(&self) -> bool {
        self.receipt.is_queued()
    }

    /// Stamp receipt strictly AFTER successful worker submission.
    pub fn mark_submitted(&mut self, pending_requests: usize) -> bool {
        self.receipt.submitted(pending_requests)
    }

    /// Observe one synchronous worker receive; retires preview only on exact
    /// terminal completion or project replacement.
    pub fn observe_receive(
        &mut self,
        generation: u64,
        project_replaced: bool,
        request_complete: bool,
    ) -> Disposition {
        self.receipt
            .observe(generation, project_replaced, request_complete)
    }
}

/// Display owner coordinating hover preview and released pending pad drop.
/// Selection/Escape cancels an active hover drag without touching an already
/// admitted pending drop; repaints show the frozen preview label until exact ACK.
#[derive(Debug, Default)]
pub struct DrumRackPadDropController {
    hover: Option<(DrumRackPadDropTarget, DrumRackPadDropPayload)>,
    pending: Option<PendingPadDropDisplay>,
}

impl DrumRackPadDropController {
    pub fn update_hover(
        &mut self,
        target: DrumRackPadDropTarget,
        payload: DrumRackPadDropPayload,
    ) -> PadDropTargetFeedback {
        let feedback = evaluate_pad_drop_target(target, &payload);
        self.hover = Some((target, payload));
        feedback
    }

    /// Cancel active drag input (e.g., Escape/blur) without retiring an already
    /// submitted drop edit in flight.
    pub fn cancel_hover(&mut self) {
        self.hover = None;
    }

    /// Release pointer over the hovered pad target, transferring custody from
    /// input capture to `PendingPadDropDisplay` so repaints never flash old
    /// canonical pad text while the worker edit is in flight.
    pub fn release_drop(&mut self) -> Result<DrumRackPadDropTarget, &'static str> {
        let Some((target, payload)) = self.hover.take() else {
            return Err("no active pad drop target");
        };
        let pending = PendingPadDropDisplay::from_release(target, payload)?;
        self.pending = Some(pending);
        Ok(target)
    }

    pub fn mark_submitted(&mut self, pending_requests: usize) -> bool {
        self.pending
            .as_mut()
            .is_some_and(|p| p.mark_submitted(pending_requests))
    }

    /// Retire a queued drop immediately when worker admission itself fails.
    pub fn retire_on_submission_refusal(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(PendingPadDropDisplay::is_queued)
        {
            self.pending = None;
        }
    }

    /// Synchronous `receive` handler: retires frozen preview on exact terminal
    /// ACK or project replacement.
    pub fn observe_receive(
        &mut self,
        generation: u64,
        project_replaced: bool,
        request_complete: bool,
    ) -> Option<Disposition> {
        let pending = self.pending.as_mut()?;
        let disposition = pending.observe_receive(generation, project_replaced, request_complete);
        if matches!(
            disposition,
            Disposition::Terminal | Disposition::Invalidated
        ) {
            self.pending = None;
        }
        Some(disposition)
    }

    /// Label rendered for `pad_id` during repaint: shows frozen pending drop
    /// preview while in flight, falling back to canonical label once retired.
    pub fn display_label_for_pad<'a>(
        &'a self,
        pad_id: NonZeroU64,
        canonical_label: &'a str,
    ) -> &'a str {
        if let Some(pending) = &self.pending
            && pending.target.pad_id == pad_id
        {
            return &pending.preview_label;
        }
        canonical_label
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_target(generation: u64, revision: u64, pad: u64) -> DrumRackPadDropTarget {
        DrumRackPadDropTarget {
            project_generation: generation,
            revision,
            source_id: None,
            device_id: NonZeroU64::new(42).unwrap(),
            pad_id: NonZeroU64::new(pad).unwrap(),
        }
    }

    #[test]
    fn accepts_supported_sample_and_instrument_and_refuses_unsupported_and_nested_racks() {
        let target = sample_target(3, 10, 1);

        assert_eq!(
            evaluate_pad_drop_target(
                target,
                &DrumRackPadDropPayload::SampleFile {
                    path: PathBuf::from("/samples/Kick_808.WAV"),
                },
            ),
            PadDropTargetFeedback::Accepted {
                pad_id: target.pad_id,
                preview_label: "kick_808".into(),
            }
        );

        assert_eq!(
            evaluate_pad_drop_target(
                target,
                &DrumRackPadDropPayload::NativeInstrument {
                    type_id: "hex.instrument.subtractive".into(),
                    display_name: "Subtractive".into(),
                },
            ),
            PadDropTargetFeedback::Accepted {
                pad_id: target.pad_id,
                preview_label: "subtractive".into(),
            }
        );

        assert!(matches!(
            evaluate_pad_drop_target(
                target,
                &DrumRackPadDropPayload::SampleFile {
                    path: PathBuf::from("/notes/readme.txt"),
                },
            ),
            PadDropTargetFeedback::Refused {
                reason: "unsupported sample file format",
                ..
            }
        ));

        assert!(matches!(
            evaluate_pad_drop_target(
                target,
                &DrumRackPadDropPayload::NativeInstrument {
                    type_id: "hex.rack.drum".into(),
                    display_name: "drum rack".into(),
                },
            ),
            PadDropTargetFeedback::Refused {
                reason: "nested racks cannot be dropped onto a drum rack pad",
                ..
            }
        ));
    }

    #[test]
    fn delayed_ack_repaint_keeps_frozen_preview_until_exact_terminal_or_project_replacement() {
        let target = sample_target(5, 22, 2);
        let mut controller = DrumRackPadDropController::default();

        // 1. Hover & cancel before release leaves no pending state.
        let _ = controller.update_hover(
            target,
            DrumRackPadDropPayload::SampleFile {
                path: PathBuf::from("/samples/snare.wav"),
            },
        );
        controller.cancel_hover();
        assert!(controller.release_drop().is_err());
        assert!(!controller.has_pending());
        assert_eq!(
            controller.display_label_for_pad(target.pad_id, "pad 2"),
            "pad 2"
        );

        // 2. Release drop -> queued preview survives intermediate repaints and unrelated completions
        //    until exact terminal ACK (zero release flicker).
        let _ = controller.update_hover(
            target,
            DrumRackPadDropPayload::SampleFile {
                path: PathBuf::from("/samples/Snare_Tight.wav"),
            },
        );
        assert_eq!(controller.release_drop(), Ok(target));
        assert!(controller.mark_submitted(2)); // 1 earlier in-flight request + this drop!

        // Repaint 1 (before any ACK): still shows frozen preview "snare_tight", NOT stale "pad 2"!
        assert_eq!(
            controller.display_label_for_pad(target.pad_id, "pad 2"),
            "snare_tight"
        );
        // Escape during flight cannot clear admitted drop custody!
        controller.cancel_hover();
        assert!(controller.has_pending());

        // Earlier request completes -> still Waiting, Repaint 2 STILL shows "snare_tight"!
        assert_eq!(
            controller.observe_receive(5, false, true),
            Some(Disposition::Waiting)
        );
        assert_eq!(
            controller.display_label_for_pad(target.pad_id, "pad 2"),
            "snare_tight"
        );

        // Exact terminal ACK arrives alongside canonical state -> retires cleanly in one receive!
        assert_eq!(
            controller.observe_receive(5, false, true),
            Some(Disposition::Terminal)
        );
        assert!(!controller.has_pending());
        assert_eq!(
            controller.display_label_for_pad(target.pad_id, "snare_tight"),
            "snare_tight"
        );

        // 3. Project replacement invalidates an in-flight drop immediately without leaking preview.
        let _ = controller.update_hover(
            target,
            DrumRackPadDropPayload::NativeInstrument {
                type_id: "hex.instrument.subtractive".into(),
                display_name: "subtractive".into(),
            },
        );
        assert_eq!(controller.release_drop(), Ok(target));
        assert!(controller.mark_submitted(1));
        assert_eq!(
            controller.observe_receive(6, true, false),
            Some(Disposition::Invalidated)
        );
        assert!(!controller.has_pending());
        assert_eq!(
            controller.display_label_for_pad(target.pad_id, "pad 2"),
            "pad 2"
        );
    }
}
