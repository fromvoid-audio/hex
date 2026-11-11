//! One resident browser-triggered clip. The immutable clip and its PCM stay
//! with the prepared revision. Only source timing moves onto the performance
//! clock, so an Arrangement loop or late publication cannot shorten the read.
use super::SessionPlayback;
use crate::{Clip, Frame, Invalid, MediaPages, Project, Track, playback::BrowserAuditionTiming};
use hex_project::TrackId;

struct Pending {
    track: TrackId,
    start: u64,
    timing: BrowserAuditionTiming,
}

#[derive(Clone, Copy)]
struct Voice {
    track: TrackId,
    start: u64,
    end: u64,
    launch: u64,
    finish: u64,
    pass: u64,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Voices {
    active: Option<Voice>,
    retired: Option<Voice>,
}

#[derive(Default)]
pub(super) struct BrowserAudition {
    pending: Option<Pending>,
    output_epoch: u64,
    pub(super) voices: Voices,
}

impl BrowserAudition {
    pub(super) fn retained_bytes(&self) -> Option<usize> {
        self.pending
            .as_ref()
            .map_or(Some(0), |pending| pending.timing.grid.retained_bytes())
    }
}

impl SessionPlayback {
    pub(crate) fn browser_audition_retained_bytes(&self) -> Option<usize> {
        self.browser_audition.retained_bytes()
    }

    pub(crate) fn arm_browser_audition(
        &mut self,
        track: TrackId,
        start: u64,
        timing: BrowserAuditionTiming,
    ) {
        let project = self.revision.arrangement();
        if audition_clip(project, track, start).is_some_and(|clip| !clip.is_streamed()) {
            self.browser_audition.pending = Some(Pending {
                track,
                start,
                timing,
            });
        }
    }
    pub(crate) fn bind_browser_audition_transport(&mut self, epoch: u64) {
        self.browser_audition.output_epoch = epoch;
    }

    /// Called once after the exact successor inherits transport. The frozen
    /// grid stays with that successor until control-side graph reclamation.
    pub(crate) fn inherit_browser_audition(&mut self, previous: &Self) {
        if !self.playing {
            return;
        }
        let project = self.revision.arrangement();
        let old = previous.revision.arrangement();
        let retain = |voice: Voice| {
            let clip = audition_clip(project, voice.track, voice.start)?;
            let original = audition_clip(old, voice.track, voice.start)?;
            let same_source = clip
                .audio
                .as_ref()?
                .shares_pcm_with(original.audio.as_ref()?)
                && clip.source_offset == original.source_offset;
            same_source.then_some(Voice {
                end: clip.end,
                ..voice
            })
        };
        let mut voices = Voices {
            active: previous.browser_audition.voices.active.and_then(retain),
            retired: previous.browser_audition.voices.retired.and_then(retain),
        };
        if let Some(pending) = &self.browser_audition.pending
            && pending.timing.transport_epoch != 0
            && pending.timing.transport_epoch == self.browser_audition.output_epoch
            && let Some(clip) = audition_clip(project, pending.track, pending.start)
        {
            let delay = if let Some(tempo) = &project.tempo_map {
                pending
                    .timing
                    .grid
                    .next_frame(tempo, self.arrangement_frame)
                    .ok()
                    .and_then(|next| {
                        if let Some(region) = self.active_loop_region()
                            && next >= region.end()
                        {
                            let wrapped = pending
                                .timing
                                .grid
                                .next_frame(tempo, region.start())
                                .ok()
                                .filter(|frame| *frame < region.end())
                                .unwrap_or(region.start());
                            (region.end() - self.arrangement_frame)
                                .checked_add(wrapped - region.start())
                        } else {
                            next.checked_sub(self.arrangement_frame)
                        }
                    })
            } else {
                Some(0)
            };
            if let Some(launch) = delay.and_then(|delay| self.performance_frame.checked_add(delay))
                && let Some(finish) = launch.checked_add(clip.end - clip.start)
            {
                voices.retired = voices
                    .active
                    .filter(|old| (old.track, old.start) != (pending.track, pending.start))
                    .map(|old| Voice {
                        finish: self.performance_frame,
                        ..old
                    });
                voices.active = Some(Voice {
                    track: pending.track,
                    start: clip.start,
                    end: clip.end,
                    launch,
                    finish,
                    pass: self.arrangement_loop_pass,
                });
            }
        }
        self.browser_audition.voices = voices;
    }
}

impl Voices {
    pub(super) fn settle(&mut self, performance: u64, arrangement: u64, pass: u64) {
        for slot in [&mut self.active, &mut self.retired] {
            if let Some(voice) = slot {
                if performance >= voice.finish
                    && (pass != voice.pass || !(voice.start..voice.end).contains(&arrangement))
                {
                    *slot = None;
                } else {
                    voice.pass = pass;
                }
            }
        }
    }

    pub(super) fn for_track(self, track: TrackId) -> Self {
        Self {
            active: self.active.filter(|voice| voice.track == track),
            retired: self.retired.filter(|voice| voice.track == track),
        }
    }

    pub(super) fn is_empty(self) -> bool {
        self.active.is_none() && self.retired.is_none()
    }

    /// Replace the authored occurrence only in the live render. The caller
    /// continues to process the raw audio through the ordinary track graph.
    pub(super) fn arrangement<P: MediaPages + ?Sized>(
        self,
        track: &Track,
        arrangement: u64,
        performance: u64,
        output: &mut [Frame],
        pages: Option<&mut P>,
        fade: Option<crate::loop_seam::LoopEndFade>,
    ) -> Result<(), Invalid> {
        track.render_raw_audio_excluding(
            arrangement,
            output,
            pages,
            [
                self.active.map(|voice| voice.start),
                self.retired.map(|voice| voice.start),
            ],
        )?;
        if let Some(fade) = fade {
            fade.apply(arrangement, output);
        }
        self.add(track, performance, output);
        Ok(())
    }

    pub(super) fn add(self, track: &Track, performance: u64, output: &mut [Frame]) {
        let Some(voice) = self.active else { return };
        let Some(clip) = clip_at(track, voice.start) else {
            return;
        };
        let offset =
            usize::try_from(voice.launch.saturating_sub(performance)).unwrap_or(usize::MAX);
        let Some(output) = output.get_mut(offset..) else {
            return;
        };
        let source_start = clip
            .start
            .saturating_add(performance.saturating_sub(voice.launch));
        let source_end = source_start.saturating_add(output.len() as u64);
        track.render_resident_clip(
            clip,
            source_start,
            source_end,
            output,
            None,
            crate::PreparedTrackMix::identity(),
        );
    }
}

fn audition_clip(project: &Project, track: TrackId, start: u64) -> Option<&Clip> {
    clip_at(
        project
            .tracks()
            .iter()
            .find(|row| row.canonical_track_id() == Some(track))?,
        start,
    )
}

fn clip_at(track: &Track, start: u64) -> Option<&Clip> {
    let index = track
        .clips
        .binary_search_by_key(&start, |clip| clip.start)
        .ok()?;
    track.clips.get(index)
}
