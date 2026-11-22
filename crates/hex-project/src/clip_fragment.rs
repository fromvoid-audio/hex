use crate::{Clip, Invalid};

/// Retain only a positive placed prefix, keeping the original fade/processing
/// domain and media available. Unlike an edge extension or a two-sided split,
/// this also permits ending in a one-shot's existing silent tail; no right-hand
/// fragment or new identity is created. Work remains on the control side.
pub(crate) fn preview_prefix_crop_with_tempo(
    original: &Clip,
    end: u64,
    tempo: &crate::TempoMap,
) -> Result<Clip, Invalid> {
    let original_end = original
        .start()
        .checked_add(original.duration())
        .ok_or(Invalid("audio prefix end overflow"))?;
    if end <= original.start() || end > original_end {
        return Err(Invalid(
            "audio prefix must retain a positive contained interval",
        ));
    }
    if end == original_end {
        return Ok(original.clone());
    }
    if original.audio_edit().is_some() {
        return crate::audio_edit::trim_view(original, crate::TrimEdge::Right, end, tempo);
    }
    let mut prefix = original.clone();
    prefix.duration = end - original.start();
    if !prefix.looping {
        prefix.source_length = prefix.source_length.min(prefix.duration);
    }
    // Retain the fade's original domain/offset, as for the left side of a split:
    // cropping must not rescale the audible prefix or move its fade-out earlier.
    Ok(prefix)
}

/// Preview an audio fragment moved to another timeline position without
/// changing its source mapping or fade domain. The fragment must be wholly
/// contained by the original clip and the destination end must be representable.
pub fn preview_clip_fragment(
    original: &Clip,
    start: u64,
    end: u64,
    destination_start: u64,
) -> Result<Clip, Invalid> {
    let original_end = original
        .start()
        .checked_add(original.duration())
        .ok_or(Invalid("timeline end overflow"))?;
    if start >= end || start < original.start() || end > original_end {
        return Err(Invalid("audio fragment must be inside the original clip"));
    }
    destination_start
        .checked_add(end - start)
        .ok_or(Invalid("fragment destination end overflow"))?;

    let mut fragment = original.clone();
    if start > fragment.start() {
        fragment = crate::edit::split_clip_at(&fragment, start, fragment.id())?.1;
    }
    if end < original_end {
        fragment = crate::edit::split_clip_at(&fragment, end, fragment.id())?.0;
    }
    fragment.start = destination_start;
    Ok(fragment)
}

/// Tempo-aware preview retaining full transformed analysis and fade context.
pub fn preview_clip_fragment_with_tempo(
    original: &Clip,
    start: u64,
    end: u64,
    destination_start: u64,
    tempo: &crate::TempoMap,
) -> Result<Clip, Invalid> {
    if original.audio_edit().is_none() {
        return preview_clip_fragment(original, start, end, destination_start);
    }
    let original_end = original
        .start()
        .checked_add(original.duration())
        .ok_or(Invalid("fragment end overflow"))?;
    if start >= end || start < original.start() || end > original_end {
        return Err(Invalid("audio fragment must be inside the original clip"));
    }
    let mut fragment = original.clone();
    if start > fragment.start() {
        fragment = crate::edit::split_clip_at_tempo(&fragment, start, fragment.id(), tempo)?.1;
    }
    if end < original_end {
        fragment = crate::edit::split_clip_at_tempo(&fragment, end, fragment.id(), tempo)?.0;
    }
    crate::audio_edit::move_clip(&mut fragment, destination_start, tempo)?;
    Ok(fragment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, Editor, Project};
    use std::path::PathBuf;

    #[test]
    fn preview_fragment_and_prefix_crop_validate_bounds_and_preserve_mapping() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: PathBuf::from("a.wav"),
                sample_rate: 48_000,
                frames: 48_000,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let Created::Clip(clip_id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 10_000,
                duration: 20_000,
                source_offset: 2_000,
                source_length: 20_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("clip")
        };

        let clip = editor.project().clip(clip_id).unwrap();
        let tempo = editor.project().tempo_map();
        assert!(preview_prefix_crop_with_tempo(clip, 10_000, tempo).is_err());
        assert!(preview_prefix_crop_with_tempo(clip, 30_001, tempo).is_err());
        let cropped = preview_prefix_crop_with_tempo(clip, 18_000, tempo).unwrap();
        assert_eq!(cropped.duration(), 8_000);
        assert_eq!(cropped.source_length(), 8_000);

        assert!(preview_clip_fragment(clip, 15_000, 15_000, 0).is_err());
        assert!(preview_clip_fragment(clip, 9_999, 20_000, 0).is_err());
        let fragment = preview_clip_fragment(clip, 14_000, 22_000, 50_000).unwrap();
        assert_eq!(fragment.start(), 50_000);
        assert_eq!(fragment.duration(), 8_000);
        assert_eq!(fragment.source_offset(), 6_000);
    }
}
