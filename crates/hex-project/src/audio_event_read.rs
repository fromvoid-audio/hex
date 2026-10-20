//! Borrowed current/default processing-root facts and virtual crop-boundary events.
//! These facts are not a domain compatibility token: consumers comparing a saved
//! payload must also compare all relevant Copy point values outside the read.
use crate::{
    AudioClipEdit, AudioDomainClock, AudioViewPlacement, BeatTime, Clip, ClipFade, Invalid,
    SourceGainPoint, Tempo, TempoMap, TempoPoint, WarpMarker, WarpMode,
};
mod events;
pub use events::EditableAudioEventsRead;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFadeProjectionFacts {
    pub sample_rate: u32,
    pub origin_beat_raw: i128,
    pub local_start_raw: u64,
    pub tempo_point_count: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioClipDomainFacts {
    pub source_offset: u64,
    pub source_length: u64,
    pub source_phase: u64,
    pub looping: bool,
    pub clock: AudioDomainClock,
    pub pitch_cents: i16,
    pub sample_bpm: Option<Tempo>,
    pub warp_enabled: bool,
    pub warp_mode: Option<WarpMode>,
    pub warp_marker_count: usize,
    pub source_gain_point_count: usize,
    pub fade: ClipFade,
    pub fade_start: u64,
    pub fade_projection: Option<AudioFadeProjectionFacts>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioClipRootFacts {
    pub domain: AudioClipDomainFacts,
    pub view_start: u64,
    pub view_end: u64,
    pub musical_start: Option<BeatTime>,
    pub placement: Option<AudioViewPlacement>,
}
impl AudioClipRootFacts {
    /// The scalar defaults used by actual control-side capture. Empty owning
    /// containers are constructed only by AudioClipEdit::capture, never here.
    pub(crate) fn capture(clip: &Clip) -> Self {
        Self {
            domain: AudioClipDomainFacts {
                source_offset: clip.source_offset,
                source_length: clip.source_length,
                source_phase: clip.source_phase,
                looping: clip.looping,
                clock: AudioDomainClock::Frames(clip.duration),
                pitch_cents: 0,
                sample_bpm: None,
                warp_enabled: false,
                warp_mode: None,
                warp_marker_count: 0,
                source_gain_point_count: 0,
                fade: clip.fade,
                fade_start: 0,
                fade_projection: None,
            },
            view_start: 0,
            view_end: clip.duration,
            musical_start: None,
            placement: None,
        }
    }
    fn retained(edit: &AudioClipEdit) -> Self {
        let domain = &edit.domain;
        Self {
            domain: AudioClipDomainFacts {
                source_offset: domain.source_offset,
                source_length: domain.source_length,
                source_phase: domain.source_phase,
                looping: domain.looping,
                clock: domain.clock,
                pitch_cents: domain.pitch_cents,
                sample_bpm: domain.sample_bpm,
                warp_enabled: domain.warp_enabled,
                warp_mode: domain.warp.as_ref().map(|w| w.mode),
                warp_marker_count: domain.warp.as_ref().map_or(0, |w| w.markers.len()),
                source_gain_point_count: domain.source_gain.len(),
                fade: domain.fade,
                fade_start: domain.fade_start,
                fade_projection: domain.fade_projection.as_ref().map(|p| {
                    AudioFadeProjectionFacts {
                        sample_rate: p.tempo.sample_rate(),
                        origin_beat_raw: p.origin_beat_raw,
                        local_start_raw: p.local_start_raw,
                        tempo_point_count: p.tempo.points().len(),
                    }
                }),
            },
            view_start: edit.view_start,
            view_end: edit.view_end,
            musical_start: edit.musical_start,
            placement: edit.placement,
        }
    }
}

/// No model/domain/list/map getter. The borrowed view cannot leave its callback.
pub struct AudioClipRootRead<'a> {
    clip: &'a Clip,
    tempo: &'a TempoMap,
}
impl Clip {
    /// Read the actual retained root, or the same scalar defaults as capture.
    /// Root construction itself does not validate an editable event sequence;
    /// that operation remains fallible and preserves its full-view shortcut.
    ///
    /// ```compile_fail
    /// use hex_project::{Clip, TempoMap};
    /// fn escape<'a>(clip: &'a Clip, tempo: &'a TempoMap) {
    ///     let _escaped = clip.with_audio_event_root(tempo, |read| read);
    /// }
    /// ```
    pub fn with_audio_event_root<R>(
        &self,
        tempo: &TempoMap,
        inspect: impl for<'read> FnOnce(AudioClipRootRead<'read>) -> R,
    ) -> R {
        inspect(AudioClipRootRead { clip: self, tempo })
    }
}
impl AudioClipRootRead<'_> {
    pub fn facts(&self) -> AudioClipRootFacts {
        self.clip.audio_edit().map_or_else(
            || AudioClipRootFacts::capture(self.clip),
            AudioClipRootFacts::retained,
        )
    }
    pub fn root_frame_at(&self, position: u64) -> Result<u64, Invalid> {
        crate::audio_preview::event_root_frame_at(self.clip, position, self.tempo)
    }
    pub fn root_position_at(&self, frame: u64) -> Result<u64, Invalid> {
        crate::audio_preview::event_root_position_at(self.clip, frame, self.tempo)
    }
    pub fn warp_markers(&self) -> impl ExactSizeIterator<Item = WarpMarker> + '_ {
        self.clip
            .audio_edit()
            .and_then(|e| e.domain.warp.as_ref())
            .map_or(&[][..], |w| w.markers.as_slice())
            .iter()
            .copied()
    }
    pub fn source_gain_points(&self) -> impl ExactSizeIterator<Item = SourceGainPoint> + '_ {
        self.clip
            .audio_edit()
            .map_or(&[][..], |e| e.domain.source_gain.as_slice())
            .iter()
            .copied()
    }
    pub fn fade_projection_tempo_points(&self) -> impl ExactSizeIterator<Item = TempoPoint> + '_ {
        self.clip
            .audio_edit()
            .and_then(|e| e.domain.fade_projection.as_ref())
            .map_or(&[][..], |p| p.tempo.points())
            .iter()
            .copied()
    }
    pub fn editable_events(&self) -> Result<EditableAudioEventsRead<'_>, Invalid> {
        EditableAudioEventsRead::new(self.clip, self.facts())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioEvent, AudioEvents, Command, Created, Editor, Project};
    use std::sync::Arc;
    fn fixture(events: AudioEvents, start: u64, end: u64) -> (Project, crate::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "events".into(),
                path: "not-opened.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let mut project = editor.project().clone();
        let current = project.clip_mut(clip).unwrap();
        current.edit = Some(Arc::new(current.audio_event_root()));
        let root = current.audio_edit_mut().unwrap();
        root.view_start = start;
        root.view_end = end;
        Arc::make_mut(&mut root.domain).events = Some(Arc::new(events));
        current.start = start;
        current.duration = end.saturating_sub(start).max(1);
        current.fade = ClipFade::neutral(current.duration).unwrap();
        (project, clip)
    }
    fn result(clip: &Clip, tempo: &TempoMap) -> Result<(u64, usize), Invalid> {
        clip.with_audio_event_root(tempo, |read| {
            read.editable_events()
                .map(|events| (events.next_id(), events.len()))
        })
    }
    #[test]
    fn canonical_maximum_source_keeps_two_virtual_boundary_fragments() {
        let source = AudioEvents {
            next_id: 1025,
            events: (0..1024)
                .map(|i| AudioEvent {
                    id: i + 1,
                    start: i * 4,
                    end: i * 4 + 4,
                    source_start: i * 4,
                })
                .collect(),
        };
        let (project, id) = fixture(source, 1, 4095);
        let project = Project::from_snapshot(project.snapshot()).unwrap();
        let clip = project.clip(id).unwrap();
        let expected = clip.editable_audio_events().unwrap();
        assert_eq!(expected.events.len(), 1026);
        assert_eq!(expected.next_id, 1027);
        clip.with_audio_event_root(project.tempo_map(), |read| {
            let events = read.editable_events().unwrap();
            assert_eq!(events.len(), 1026);
            assert_eq!(events.next_id(), 1027);
            assert!(events.iter().eq(expected.events.iter().copied()));
            let mut iter = events.iter();
            assert_eq!(
                iter.next(),
                Some(AudioEvent {
                    id: 1,
                    start: 0,
                    end: 1,
                    source_start: 0
                })
            );
            assert_eq!(
                iter.next(),
                Some(AudioEvent {
                    id: 1025,
                    start: 1,
                    end: 4,
                    source_start: 1
                })
            );
            assert_eq!(
                iter.last(),
                Some(AudioEvent {
                    id: 1026,
                    start: 4095,
                    end: 4096,
                    source_start: 4095
                })
            );
        });
    }
    #[test]
    fn cropped_geometry_errors_precede_range_errors_and_full_view_keeps_shortcut() {
        let bad = AudioEvents {
            next_id: 3,
            events: vec![
                AudioEvent {
                    id: 1,
                    start: 0,
                    end: 100,
                    source_start: 0,
                },
                AudioEvent {
                    id: 1,
                    start: 100,
                    end: 4096,
                    source_start: 100,
                },
            ],
        };
        let (mut project, id) = fixture(bad, 40, 20);
        let clip = project.clip(id).unwrap();
        let error = clip.editable_audio_events().unwrap_err();
        assert_eq!(result(clip, project.tempo_map()), Err(error.clone()));
        assert_eq!(
            error.0,
            "audio events must have unique identities and bounded nonoverlapping intervals"
        );
        let root = project.clip_mut(id).unwrap().audio_edit_mut().unwrap();
        Arc::make_mut(Arc::make_mut(&mut root.domain).events.as_mut().unwrap()).events[1].id = 2;
        let clip = project.clip(id).unwrap();
        assert_eq!(
            result(clip, project.tempo_map()),
            Err(Invalid("invalid audio event split range"))
        );
        assert_eq!(
            clip.editable_audio_events().unwrap_err().0,
            "invalid audio event split range"
        );
        let root = project.clip_mut(id).unwrap().audio_edit_mut().unwrap();
        Arc::make_mut(Arc::make_mut(&mut root.domain).events.as_mut().unwrap()).next_id = 0;
        let clip = project.clip(id).unwrap();
        assert_eq!(
            result(clip, project.tempo_map()),
            Err(Invalid("audio event identity is invalid"))
        );
        let root = project.clip_mut(id).unwrap().audio_edit_mut().unwrap();
        root.view_start = 0;
        root.view_end = 4096;
        let clip = project.clip(id).unwrap();
        assert_eq!(result(clip, project.tempo_map()), Ok((0, 2)));
        assert_eq!(clip.editable_audio_events().unwrap().next_id, 0);
    }
    #[test]
    fn first_and_second_split_identity_exhaustion_match_owning_errors_without_changes() {
        for next_id in [u64::MAX, u64::MAX - 1] {
            let (project, id) = fixture(
                AudioEvents {
                    next_id,
                    events: vec![AudioEvent {
                        id: 1,
                        start: 0,
                        end: 4096,
                        source_start: 0,
                    }],
                },
                20,
                40,
            );
            let before = project.snapshot();
            let clip = project.clip(id).unwrap();
            assert_eq!(
                result(clip, project.tempo_map()),
                Err(Invalid("audio event identity exhausted"))
            );
            assert_eq!(
                clip.editable_audio_events().unwrap_err().0,
                "audio event identity exhausted"
            );
            assert_eq!(project.snapshot(), before);
        }
    }
    #[test]
    fn noncanonical_oversized_source_refusal_is_read_only_and_full_shortcut_unchanged() {
        let source = AudioEvents {
            next_id: 1026,
            events: (0..1025)
                .map(|i| AudioEvent {
                    id: i + 1,
                    start: i * 2,
                    end: i * 2 + 2,
                    source_start: i * 2,
                })
                .collect(),
        };
        let (mut project, id) = fixture(source, 1, 2049);
        assert!(
            Project::from_snapshot(project.snapshot()).is_err(),
            "canonical admission still rejects an oversized authored source"
        );
        let clip = project.clip(id).unwrap();
        assert_eq!(
            clip.editable_audio_events().unwrap().events.len(),
            1027,
            "owning helper keeps its existing transient geometry behavior"
        );
        assert_eq!(
            result(clip, project.tempo_map()),
            Err(Invalid("audio event read exceeds canonical event limit"))
        );
        let root = project.clip_mut(id).unwrap().audio_edit_mut().unwrap();
        root.view_start = 0;
        root.view_end = 4096;
        let clip = project.clip(id).unwrap();
        assert_eq!(result(clip, project.tempo_map()), Ok((1026, 1025)));
        assert_eq!(clip.editable_audio_events().unwrap().events.len(), 1025);
    }
}
