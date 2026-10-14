//! One known constructor family; declaration is not physical allocator usage.
use std::{alloc::Layout, sync::atomic::AtomicUsize};
#[derive(Clone, Copy, Debug)]
pub(crate) struct PcmArcReceipt {
    len: usize,
    payload_bytes: usize,
    overhead_bytes: usize,
}
#[derive(Clone, Copy, Debug)]
enum PcmArcOrigin {
    ValidatedNonemptyVec,
    Unknown,
}
impl PcmArcReceipt {
    pub(crate) fn current(len: usize) -> Option<Self> {
        let known = native_layout_known();
        Self::capture(
            len,
            if known {
                PcmArcOrigin::ValidatedNonemptyVec
            } else {
                PcmArcOrigin::Unknown
            },
            known,
        )
    }
    fn capture(len: usize, origin: PcmArcOrigin, gate: bool) -> Option<Self> {
        if !gate || len == 0 || !matches!(origin, PcmArcOrigin::ValidatedNonemptyVec) {
            return None;
        }
        let payload = Layout::array::<crate::Frame>(len).ok()?;
        let requested = Layout::array::<AtomicUsize>(2)
            .ok()?
            .extend(payload)
            .ok()?
            .0
            .pad_to_align();
        Some(Self {
            len,
            payload_bytes: payload.size(),
            overhead_bytes: requested.size().checked_sub(payload.size())?,
        })
    }
    pub(crate) fn checked_parts(self, len: usize) -> Option<(usize, usize)> {
        (self.len == len).then_some((self.payload_bytes, self.overhead_bytes))
    }
}
pub(crate) fn native_layout_known() -> bool {
    let native = match option_env!("HEX_PCM_ARC_LAYOUT_TARGET") {
        Some("x86_64-unknown-linux-gnu") => cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )),
        Some("aarch64-apple-darwin") => cfg!(all(target_arch = "aarch64", target_os = "macos")),
        _ => false,
    };
    native && option_env!("HEX_PCM_ARC_LAYOUT_KNOWN") == Some("1")
}

#[cfg(test)]
#[path = "../build_support/pcm_gate.rs"]
mod build_gate_tests;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_origin_empty_gate_and_overflow_stay_unknown() {
        assert!(PcmArcReceipt::capture(1, PcmArcOrigin::Unknown, true).is_none());
        assert!(PcmArcReceipt::capture(0, PcmArcOrigin::ValidatedNonemptyVec, true).is_none());
        assert!(PcmArcReceipt::capture(1, PcmArcOrigin::ValidatedNonemptyVec, false).is_none());
        assert!(
            PcmArcReceipt::capture(usize::MAX, PcmArcOrigin::ValidatedNonemptyVec, true).is_none()
        );
        let receipt = PcmArcReceipt::capture(5, PcmArcOrigin::ValidatedNonemptyVec, true).unwrap();
        assert_eq!(
            receipt.checked_parts(5).unwrap().0,
            5 * std::mem::size_of::<crate::Frame>()
        );
        assert!(receipt.checked_parts(4).is_none());
    }
}
#[cfg(test)]
mod constructor_tests {
    use crate::storage_report::{
        PlaybackStorageVisitor, StorageCharge, StorageItem, StorageOwner, StorageReportError,
    };
    use crate::{Audio, Clip, Project, Track, playback::Playback};
    use std::sync::Arc;
    struct Sink<'a> {
        items: [Option<StorageItem<'a>>; 128],
        len: usize,
    }
    impl<'a> PlaybackStorageVisitor<'a> for Sink<'a> {
        fn visit(&mut self, item: StorageItem<'a>) -> Result<(), StorageReportError> {
            if self.len == self.items.len() {
                return Err(StorageReportError::SinkCapacity);
            }
            self.items[self.len] = Some(item);
            self.len += 1;
            Ok(())
        }
    }
    #[test]
    fn actual_validated_constructor_clone_identity_parent_charge_and_other_unknowns() {
        assert!(Audio::new(48_000, Vec::new()).is_err());
        assert!(Audio::new(0, vec![[0.0; 2]]).is_err());
        assert!(Audio::new(48_000, vec![[f32::NAN, 0.0]]).is_err());
        let mut frames = Vec::with_capacity(91);
        frames.resize(5, [0.25, -0.25]);
        let audio = Audio::new(48_000, frames).unwrap();
        let clone = audio.clone();
        assert!(audio.shares_pcm_with(&clone));
        let known = option_env!("HEX_PCM_ARC_LAYOUT_KNOWN") == Some("1");
        assert_eq!(audio.pcm_arc_receipt.is_some(), known);
        assert_eq!(clone.pcm_arc_receipt.is_some(), known);
        if let Some(receipt) = audio.pcm_arc_receipt {
            let (payload, overhead) = receipt.checked_parts(5).unwrap();
            assert_eq!(payload, 5 * std::mem::size_of::<crate::Frame>());
            assert!(overhead > 0);
            assert_eq!(
                clone.pcm_arc_receipt.unwrap().checked_parts(5),
                Some((payload, overhead))
            );
        }
        let distinct = Audio::new(48_000, vec![[0.25, -0.25]; 5]).unwrap();
        assert!(!audio.shares_pcm_with(&distinct));
        // Raw/default/future origin has no receipt, even on a supported build.
        let untracked = Audio {
            sample_rate: 48_000,
            frames: Arc::from(vec![[0.25, -0.25]; 5]),
            pcm_arc_receipt: None,
        };
        let mut clips = Vec::with_capacity(11);
        for value in [audio, clone, distinct, untracked] {
            clips.push(Clip::new(value, 0, 5, false).unwrap());
        }
        let mut playback = Playback::new(
            Project::new(48_000, vec![Track::new(clips, 1.0, false).unwrap()]).unwrap(),
        );
        let mut sink = Sink {
            items: [None; 128],
            len: 0,
        };
        let summary = playback.visit_storage_on_control(&mut sink).unwrap();
        assert!(summary.require_complete().is_err());
        let parent = sink.items[..sink.len]
            .iter()
            .flatten()
            .find(|i| i.location.component == "track.clips")
            .unwrap();
        assert!(
            matches!(parent.charge, StorageCharge::MeasuredData { capacity: 11, element_bytes, bytes }
            if element_bytes == std::mem::size_of::<Clip>() && bytes == 11 * std::mem::size_of::<Clip>())
        );
        let mut declared = 0;
        let mut unknown_pcm = 0;
        for item in sink.items[..sink.len].iter().flatten() {
            if matches!(item.owner, StorageOwner::Shared(owner) if owner.category() == "resident_pcm")
            {
                match item.charge {
                    StorageCharge::OwnerOverheadDeclared { .. } => declared += 1,
                    StorageCharge::Unknown(_) => unknown_pcm += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(declared, if known { 3 } else { 0 });
        assert_eq!(unknown_pcm, if known { 1 } else { 4 });
    }
}
#[cfg(test)]
mod refusal_tests {
    use crate::storage_report::{
        PlaybackStorageVisitor, StorageItem, StorageOwner, StorageReportError,
    };
    use crate::{Audio, Clip, Project, Track, playback::Playback};
    struct Refuse {
        component: &'static str,
    }
    impl<'a> PlaybackStorageVisitor<'a> for Refuse {
        fn visit(&mut self, item: StorageItem<'a>) -> Result<(), StorageReportError> {
            if matches!(item.owner, StorageOwner::Shared(owner) if owner.category() == "resident_pcm")
                && item.location.component == self.component
            {
                return Err(StorageReportError::SinkCapacity);
            }
            Ok(())
        }
    }
    #[test]
    fn each_pcm_emission_refusal_preserves_clock_and_actual_owners() {
        let audio = Audio::new(48_000, vec![[0.2, -0.2]; 8]).unwrap();
        let mut playback = Playback::new(
            Project::new(
                48_000,
                vec![
                    Track::new(
                        vec![Clip::new(audio.clone(), 0, 8, false).unwrap()],
                        1.0,
                        false,
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
        );
        playback.apply(crate::PlaybackCommand::Seek(3));
        let strong = std::sync::Arc::strong_count(&audio.frames);
        for component in ["arc.pointee", "arc.owner_overhead"] {
            assert_eq!(
                playback.visit_storage_on_control(&mut Refuse { component }),
                Err(StorageReportError::SinkCapacity)
            );
            assert_eq!(playback.position(), 3);
            assert_eq!(std::sync::Arc::strong_count(&audio.frames), strong);
        }
    }
}
