//! Arrangement output replacement metadata. Original inputs retain their device
//! history; a printed clip is mixed after devices and before the track mixer.
//! Suppression belongs to the track, independently of any printed clip, so
//! deleting a print leaves silence instead of revealing the retained input.
use crate::Invalid;

/// Processing position of a top-level Arrangement audio clip. This is local to
/// the placement, never shared alias source content or a Console playback mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AudioClipRole {
    #[default]
    Input,
    PostDevice,
}

/// Checked half-open project-frame interval of suppressed original output.
/// Native/foreign device processing continues throughout this interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputSuppression {
    start: u64,
    end: u64,
}
impl OutputSuppression {
    pub fn new(start: u64, end: u64) -> Result<Self, Invalid> {
        if start >= end {
            return Err(Invalid(
                "output suppression needs a positive frame interval",
            ));
        }
        Ok(Self { start, end })
    }
    pub fn start(self) -> u64 {
        self.start
    }
    pub fn end(self) -> u64 {
        self.end
    }
    pub fn contains(self, frame: u64) -> bool {
        self.start <= frame && frame < self.end
    }
}

/// One interval per possible print. Edits merge touching intervals; document
/// admission requires that canonical order rather than repairing saved state.
pub const MAX_OUTPUT_SUPPRESSIONS: usize = crate::MAX_CLIPS;

pub(crate) fn validate(spans: &[OutputSuppression]) -> Result<(), Invalid> {
    if spans.len() > MAX_OUTPUT_SUPPRESSIONS
        || spans.windows(2).any(|pair| pair[0].end >= pair[1].start)
    {
        return Err(Invalid(
            "output suppression intervals must be bounded and disjoint",
        ));
    }
    Ok(())
}

pub(crate) fn insert(
    spans: &mut Vec<OutputSuppression>,
    mut incoming: OutputSuppression,
) -> Result<(), Invalid> {
    let first = spans.partition_point(|span| span.end < incoming.start);
    let mut last = first;
    while let Some(span) = spans.get(last).filter(|span| span.start <= incoming.end) {
        incoming.start = incoming.start.min(span.start);
        incoming.end = incoming.end.max(span.end);
        last += 1;
    }
    if first == last {
        if spans.len() == MAX_OUTPUT_SUPPRESSIONS {
            return Err(Invalid("output suppression interval limit reached"));
        }
        spans
            .try_reserve(1)
            .map_err(|_| Invalid("could not reserve output suppression interval"))?;
        spans.insert(first, incoming);
    } else {
        spans[first] = incoming;
        spans.drain(first + 1..last);
    }
    Ok(())
}

/// Adopt only a validated, durably prepared exact-range render. Existing input
/// placements are untouched; only earlier prints in the interval are replaced.
pub(crate) fn adopt(
    project: &mut crate::Project,
    file: crate::BounceFile,
) -> Result<crate::ClipId, Invalid> {
    let end = file
        .start
        .checked_add(file.frames)
        .ok_or(Invalid("printed output end overflows"))?;
    let span = OutputSuppression::new(file.start, end)?;
    let index = project
        .tracks
        .iter()
        .position(|track| track.id == file.track)
        .ok_or(Invalid("printed output track changed"))?;
    let previous: Vec<_> = project.tracks[index]
        .clips()
        .iter()
        .filter(|clip| {
            clip.role == AudioClipRole::PostDevice
                && clip.start < end
                && clip.start + clip.duration > file.start
        })
        .cloned()
        .collect();
    for original in previous {
        let old_end = original.start + original.duration;
        let alias = project.clip_alias(original.id);
        let mut kept = Vec::new();
        if original.start < file.start {
            let (left, _) = crate::edit::split_clip_at_tempo(
                &original,
                file.start,
                original.id,
                project.tempo_map(),
            )?;
            kept.push(left);
        }
        if old_end > end {
            let id = if kept.is_empty() {
                original.id
            } else {
                crate::ClipId(project.allocate_id()?)
            };
            let (_, right) =
                crate::edit::split_clip_at_tempo(&original, end, id, project.tempo_map())?;
            kept.push(right);
        }
        project.tracks[index]
            .content
            .audio_clips
            .retain(|clip| clip.id != original.id);
        for clip in kept {
            if let Some(alias) = alias {
                project.aliases.members.insert(clip.id, alias);
            }
            project.tracks[index].content.audio_clips.push(clip);
        }
    }
    let asset = project.add_prepared_asset(
        file.path,
        project.sample_rate(),
        file.frames,
        file.frames,
        crate::IDENTITY_CONVERSION,
    )?;
    let id = crate::ClipId(project.allocate_id()?);
    let clip = crate::Clip {
        role: AudioClipRole::PostDevice,
        active: true,
        name: "bounce".into(),
        flavor: crate::ClipFlavor::default(),
        id,
        asset,
        start: file.start,
        duration: file.frames,
        source_offset: 0,
        source_length: file.frames,
        looping: false,
        source_phase: 0,
        fade: crate::ClipFade::neutral(file.frames)?,
        gain: crate::ClipGain::UNITY,
        edit: None,
    };
    let track = &mut project.tracks[index];
    insert(&mut track.output_suppression, span)?;
    track
        .content
        .audio_clips
        .try_reserve(1)
        .map_err(|_| Invalid("could not reserve printed output clip"))?;
    track.content.audio_clips.push(clip);
    track
        .content
        .audio_clips
        .sort_unstable_by_key(|clip| (clip.start, clip.id));
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BounceFile, BounceSource, BounceTarget, Command, Created, Editor, Project, TimeRange,
    };

    #[test]
    fn suppression_union_retains_silent_gaps_and_half_open_boundaries() {
        let mut spans = Vec::new();
        for (start, end) in [(20, 30), (4, 8), (12, 18), (8, 12), (28, 40)] {
            insert(&mut spans, OutputSuppression::new(start, end).unwrap()).unwrap();
        }
        assert_eq!(
            spans,
            vec![
                OutputSuppression::new(4, 18).unwrap(),
                OutputSuppression::new(20, 40).unwrap()
            ]
        );
        assert!(spans[0].contains(4));
        assert!(!spans[0].contains(18));
        insert(&mut spans, OutputSuppression::new(18, 20).unwrap()).unwrap();
        assert_eq!(spans, vec![OutputSuppression::new(4, 40).unwrap()]);
        assert!(OutputSuppression::new(9, 9).is_err());
        assert!(OutputSuppression::new(u64::MAX, 0).is_err());
        assert!(
            validate(&[
                OutputSuppression::new(4, 8).unwrap(),
                OutputSuppression::new(8, 9).unwrap()
            ])
            .is_err()
        );
    }

    #[test]
    fn range_print_replacement_and_deletion_preserve_originals_and_history() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported {
            track,
            clip: original,
            ..
        } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("audio track")
        };
        let source = editor.project().clip(original).unwrap().clone();
        let print = |start, end| Command::CommitArrangementBounce {
            target: BounceTarget::Range(TimeRange::new(vec![track], start, end).unwrap()),
            in_place: true,
            files: vec![BounceFile {
                source: BounceSource::Arrangement,
                track,
                start,
                frames: end - start,
                path: format!("print-{start}-{end}.wav").into(),
            }],
        };
        editor.execute(print(20, 80)).unwrap();
        editor.execute(print(40, 60)).unwrap();
        let project = editor.project();
        assert_eq!(project.clip(original), Some(&source));
        assert_eq!(
            project.track(track).unwrap().output_suppression(),
            &[OutputSuppression::new(20, 80).unwrap()]
        );
        let prints: Vec<_> = project
            .track(track)
            .unwrap()
            .clips()
            .iter()
            .filter(|clip| clip.role() == AudioClipRole::PostDevice)
            .map(|clip| (clip.id(), clip.start(), clip.duration()))
            .collect();
        assert_eq!(
            prints
                .iter()
                .map(|(_, start, len)| (*start, *len))
                .collect::<Vec<_>>(),
            vec![(20, 20), (40, 20), (60, 20)]
        );
        assert_eq!(
            Project::from_snapshot(project.snapshot()).unwrap(),
            *project
        );
        for (id, _, _) in prints {
            editor.execute(Command::RemoveClip(id)).unwrap();
        }
        assert_eq!(editor.project().track(track).unwrap().clips(), &[source]);
        assert_eq!(
            editor.project().track(track).unwrap().output_suppression(),
            &[OutputSuppression::new(20, 80).unwrap()]
        );
        let deleted = editor.project().clone();
        assert!(editor.undo());
        assert!(editor.redo());
        assert_eq!(editor.project(), &deleted);
        let mut invalid = deleted.snapshot();
        invalid.tracks[0].output_suppression = vec![
            OutputSuppression::new(20, 80).unwrap(),
            OutputSuppression::new(70, 90).unwrap(),
        ];
        assert!(Project::from_snapshot(invalid).is_err());
    }
}
