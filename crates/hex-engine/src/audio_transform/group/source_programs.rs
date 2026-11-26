//! A group can contain many cropped copies of one retained source. Compile
//! each equivalent sibling once, then serve its independently editable IDs.
use super::{AudioDomainClock, AudioViewProjection, GroupSource, Invalid, MAX_AUDIO_DOMAIN_FRAMES};
use hex_project::RetainedAudioRoot;

pub(super) struct SourceProgram<'a, 'media> {
    pub source: &'a GroupSource<'media>,
    pub descendants: &'a [GroupSource<'media>],
    pub ids: Vec<u64>,
}

fn same_media(left: &GroupSource<'_>, right: &GroupSource<'_>) -> bool {
    std::ptr::eq(left.asset, right.asset)
        && left.audio.sample_rate() == right.audio.sample_rate()
        && left.audio.shares_pcm_with(right.audio)
}

impl SourceProgram<'_, '_> {
    fn matches(&self, source: &GroupSource<'_>, descendants: &[GroupSource<'_>]) -> bool {
        // A provenance ID can also belong to a fork with different edits.
        self.source.root.clip.id() == source.root.clip.id()
            && same_media(self.source, source)
            && self.source.root.clip == source.root.clip
            && self.source.root.tempo == source.root.tempo
            && self.descendants.len() == descendants.len()
            && self
                .descendants
                .iter()
                .zip(descendants)
                .all(|(a, b)| same_media(a, b))
    }
}

pub(super) fn source_programs<'a, 'media>(
    roots: &[RetainedAudioRoot],
    mut remaining: &'a [GroupSource<'media>],
) -> Result<Vec<SourceProgram<'a, 'media>>, Invalid> {
    let mut programs: Vec<SourceProgram<'a, 'media>> = Vec::new();
    programs
        .try_reserve_exact(roots.len())
        .map_err(|_| Invalid("grouped source plan allocation failed"))?;
    for root in roots {
        let (source, rest) = remaining
            .split_first()
            .filter(|(source, _)| std::ptr::eq(source.root, root))
            .ok_or(Invalid("grouped audio dependency missing"))?;
        let (descendants, rest) = rest
            .split_at_checked(root.clip.retained_audio_roots().count())
            .ok_or(Invalid("grouped audio dependency tree is incomplete"))?;
        remaining = rest;
        if let Some(program) = programs.iter_mut().find(|p| p.matches(source, descendants)) {
            program
                .ids
                .try_reserve(1)
                .map_err(|_| Invalid("grouped source plan allocation failed"))?;
            program.ids.push(root.id);
        } else {
            programs.push(SourceProgram {
                source,
                descendants,
                ids: vec![root.id],
            });
        }
    }
    if !remaining.is_empty() {
        return Err(Invalid(
            "grouped audio dependency tree differs from its roots",
        ));
    }
    for program in &mut programs {
        program.ids.sort_unstable();
    }
    Ok(programs)
}

/// Count each processing program once at its own level, including its
/// distinct nested dependencies, before reserving any PCM.
pub(super) fn admit_work(
    programs: &[SourceProgram<'_, '_>],
    mut work: u64,
) -> Result<u64, Invalid> {
    for program in programs {
        let source = program.source;
        let root = &source.root.clip;
        let root_work = if let Some(inner) = root.audio_edit() {
            inner.validate(root, &source.root.tempo, source.asset.playback_frames())?;
            if inner.placement.is_some() || inner.content_view.is_some() {
                let view = AudioViewProjection::new(root, &source.root.tempo)?;
                if view.occurrence_count() > 8_192 {
                    return Err(Invalid("grouped source exceeds bounded placement work"));
                }
                let mut frames = root.duration();
                for index in 0..view.occurrence_count() {
                    if index == 0 || matches!(inner.domain.clock, AudioDomainClock::Beats(_)) {
                        frames = frames
                            .checked_add(view.occurrence(index)?.root_frame_length)
                            .ok_or(Invalid("grouped source preparation work overflow"))?;
                    }
                }
                frames
            } else {
                root.duration()
                    .checked_add(inner.projected_domain(&source.root.tempo)?.1)
                    .ok_or(Invalid("grouped source preparation work overflow"))?
            }
        } else {
            root.duration()
        };
        work = work
            .checked_add(root_work)
            .filter(|work| *work <= MAX_AUDIO_DOMAIN_FRAMES)
            .ok_or(Invalid(
                "grouped audio exceeds bounded aggregate source preparation work",
            ))?;
        if !program.descendants.is_empty() {
            let children = root
                .audio_edit()
                .and_then(|edit| edit.domain.events.as_ref())
                .ok_or(Invalid("grouped audio dependency tree is incomplete"))?;
            let nested = source_programs(&children.sources, program.descendants)?;
            work = admit_work(&nested, work)?;
        }
    }
    Ok(work)
}
