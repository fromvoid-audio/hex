//! Explicit borrowed sample capability for bounded control-side display work.
use super::Waveform;
use std::ops::Range;

impl Waveform {
    /// Borrow genuine retained samples in this summary's contiguous frame clock.
    /// Refuses peak-only, repeated and sparse storage, invalid/empty intervals,
    /// or a range longer than `max_frames` before borrowing the slice. `None`
    /// means unavailable, never silence. Authentic zero samples remain present.
    ///
    /// This performs no decode, allocation or copying. The caller must bound the
    /// combined sample work across its viewport, including multiple queries;
    /// `max_frames` bounds this window only. Never use it on the audio callback.
    pub fn bounded_samples(&self, range: Range<usize>, max_frames: usize) -> Option<&[[f32; 2]]> {
        let length = range.end.checked_sub(range.start)?;
        if length == 0
            || length > max_frames
            || range.end > self.source_frames
            || self.repeat.is_some()
            || self.sparse.is_some()
        {
            return None;
        }
        self.samples.as_ref()?.frames().get(range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::{Repeat, summarize, summarize_repeated, summarize_sparse};
    use hex_engine::{Audio, PreparedRevision, SineTable};
    use hex_project::{AudioEventEdit, Command, Created, Editor, Project};
    use std::sync::Arc;

    #[test]
    fn windows_borrow_original_samples_and_keep_zero_distinct_from_unavailable() {
        let audio = Audio::new(44_100, vec![[0.0; 2], [2.0, -3.0], [-0.0, 0.0], [0.5; 2]]).unwrap();
        let waveform = summarize(&audio, 1).unwrap();
        let samples = waveform.bounded_samples(1..3, 2).unwrap();
        assert_eq!(samples, &audio.frames()[1..3]);
        assert_eq!(samples.as_ptr(), audio.frames()[1..3].as_ptr());
        assert_eq!(waveform.bounded_samples(0..1, 1), Some(&[[0.0; 2]][..]));
        let peak_only = crate::waveform::summarize_peaks(&audio, 1).unwrap();
        assert!(peak_only.bounded_samples(0..1, 1).is_none());
        for (range, limit) in [
            (0..4, 3),
            (0..1, 0),
            (1..1, 1),
            (4..5, 1),
            (0..usize::MAX, usize::MAX),
            (Range { start: 2, end: 1 }, 1),
        ] {
            assert!(waveform.bounded_samples(range, limit).is_none());
        }
    }
    #[test]
    fn repeat_and_sparse_storage_do_not_masquerade_as_contiguous_samples() {
        let audio = Audio::new(44_100, vec![[0.25; 2]; 4]).unwrap();
        let repeated = summarize_repeated(
            &audio,
            Repeat {
                offset: 0,
                period: 4,
                phase: 0,
            },
            8,
            2,
        )
        .unwrap();
        assert!(repeated.bounded_samples(0..1, 1).is_none());
        // Obtain an authentic packed layout through its public preparation owner.
        let mut editor = Editor::new(Project::new(44_100).unwrap(), 4).unwrap();
        let Created::Imported { clip, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 44_100,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Place {
                    event: 1,
                    start: 1_000_000,
                },
            })
            .unwrap();
        let prepared = PreparedRevision::prepare_with_audio_transforms(
            editor.project(),
            &std::collections::BTreeMap::from([(asset, audio)]),
            Arc::new(SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
        )
        .unwrap();
        let view = &prepared.arrangement().audio_clip_views()[0];
        let sparse =
            summarize_sparse(&view.audio, Arc::clone(view.sparse.as_ref().unwrap()), 2).unwrap();
        assert!(sparse.bounded_samples(1_000_000..1_000_001, 1).is_none());
    }
}
