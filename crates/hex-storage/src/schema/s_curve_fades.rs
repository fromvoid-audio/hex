//! Schema113 layers explicit per-ramp S curves over the latest compatible
//! historical document. Existing documents retain their exact coordinates and
//! gain law. Old writers stay byte-compatible for projects without S curves.
//! The sparse inventory addresses arrangement, event and retained-source fades
//! by their canonical IDs, not by display order. Migration is in memory.
use crate::{Project, S_CURVE_FADE_SCHEMA_VERSION, StorageError};
use hex_project::{AudioEventFade, Clip, ClipFade, ClipSnapshot, FadeCurveLaw, Snapshot};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::{collections::BTreeMap, sync::Arc};

const LEGACY: [FadeCurveLaw; 2] = [FadeCurveLaw::Linear; 2];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<'a> {
    format: &'a str,
    version: u32,
    #[serde(borrow)]
    base: &'a RawValue,
    fades: Vec<Row>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    sources: Vec<u64>,
    slot: Slot,
    curve: Curve,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Slot {
    Clip,
    Domain,
    Event(u64),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Curve {
    smooth: [bool; 2],
    in_start: u64,
    out_end: u64,
    bends: [i16; 2],
    positions: [u16; 2],
}

impl Curve {
    pub(super) fn capture(fade: ClipFade) -> Self {
        Self {
            smooth: fade.curve_laws().map(|law| law == FadeCurveLaw::SmoothStep),
            in_start: fade.in_start(),
            out_end: fade.out_end_offset(),
            bends: [fade.in_curve(), fade.out_curve()],
            positions: [fade.in_position(), fade.out_position()],
        }
    }

    pub(super) fn restore(self, fade: ClipFade) -> Result<ClipFade, StorageError> {
        let [in_curve, out_curve] = self.bends;
        let [in_position, out_position] = self.positions;
        let fade = fade
            .with_shape(self.in_start, self.out_end, in_curve, out_curve)
            .and_then(|fade| fade.with_curve_positions(in_position, out_position))
            .map_err(invalid)?;
        Ok(fade.with_curve_laws(self.smooth.map(|smooth| {
            if smooth {
                FadeCurveLaw::SmoothStep
            } else {
                FadeCurveLaw::Linear
            }
        })))
    }
}

fn invalid(error: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(error.0)
}

fn clip_has_curve(clip: &Clip) -> bool {
    clip.fade().curve_laws() != LEGACY
        || clip.retained_fade().curve_laws() != LEGACY
        || clip
            .audio_edit()
            .and_then(|edit| edit.domain.events.as_ref())
            .is_some_and(|sequence| {
                sequence.events.iter().any(|event| {
                    event
                        .fade
                        .is_some_and(|fade| fade.shape().curve_laws() != LEGACY)
                })
            })
}

pub(crate) fn contains(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(|clip| {
            clip_has_curve(clip)
                || clip
                    .retained_audio_roots()
                    .any(|root| clip_has_curve(&root.clip))
        })
}

/// Both sides of the codec use the same address and project validation. A
/// retained root is reconstructed only if its fade or descendants changed.
fn visit(
    project: &Project,
    snapshot: &mut Snapshot,
    mut edit: impl FnMut(u64, &[u64], Slot, ClipFade) -> Result<ClipFade, StorageError>,
) -> Result<(), StorageError> {
    type Edit<'a> = dyn FnMut(u64, &[u64], Slot, ClipFade) -> Result<ClipFade, StorageError> + 'a;
    fn visit_clip(
        project: &Project,
        clip: &mut ClipSnapshot,
        owner: u64,
        sources: &mut Vec<u64>,
        edit: &mut Edit<'_>,
    ) -> Result<bool, StorageError> {
        let next = edit(owner, sources, Slot::Clip, clip.fade)?;
        let mut changed = next != clip.fade;
        clip.fade = next;
        let Some(domain) = clip
            .edit
            .as_mut()
            .map(|view| Arc::make_mut(&mut view.domain))
        else {
            return Ok(changed);
        };
        let next = edit(owner, sources, Slot::Domain, domain.fade)?;
        changed |= next != domain.fade;
        domain.fade = next;
        if let Some(sequence) = &mut domain.events {
            let sequence = Arc::make_mut(sequence);
            for event in &mut sequence.events {
                if let Some(fade) = event.fade {
                    let next = edit(owner, sources, Slot::Event(event.id), fade.shape())?;
                    if next != fade.shape() {
                        event.fade =
                            Some(AudioEventFade::new(fade.source_start(), next).map_err(invalid)?);
                        changed = true;
                    }
                }
            }
            for root in &mut sequence.sources {
                let mut snapshot = root.clip.retained_snapshot();
                sources.push(root.id);
                let edited = visit_clip(project, &mut snapshot, owner, sources, edit)?;
                sources.pop();
                if edited {
                    *root = project
                        .restore_retained_audio_root(root.id, snapshot, Arc::clone(&root.tempo))
                        .map_err(invalid)?;
                    changed = true;
                }
            }
        }
        Ok(changed)
    }
    for track in &mut snapshot.tracks {
        if let Ok(clips) = track.audio_clips_mut() {
            for clip in clips {
                visit_clip(project, clip, clip.id, &mut Vec::new(), &mut edit)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    let mut snapshot = project.snapshot();
    let mut fades = Vec::new();
    visit(project, &mut snapshot, |clip, sources, slot, fade| {
        if fade.curve_laws() != LEGACY {
            fades.push(Row {
                clip,
                sources: sources.to_vec(),
                slot,
                curve: Curve::capture(fade),
            });
        }
        Ok(fade.with_curve_laws(LEGACY))
    })?;
    let prior = Project::from_snapshot(snapshot).map_err(invalid)?;
    let base = crate::encode_prior(&prior)?;
    crate::encode_document(&Document {
        format: "hex-project",
        version: S_CURVE_FADE_SCHEMA_VERSION,
        base: serde_json::from_slice(&base)?,
        fades,
    })
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.len() > crate::MAX_PROJECT_DOCUMENT_BYTES {
        return Err(StorageError::Invalid(
            "project exceeds the document size limit",
        ));
    }
    let document: Document<'_> = serde_json::from_slice(bytes)?;
    #[derive(Deserialize)]
    struct Header {
        version: u32,
    }
    let base = document.base.get().as_bytes();
    let header: Header = serde_json::from_slice(base)?;
    // This envelope is never nested, even if a future reader adds another law.
    if header.version >= S_CURVE_FADE_SCHEMA_VERSION || document.fades.is_empty() {
        return Err(StorageError::Invalid("invalid s-curve fade envelope"));
    }
    let project = crate::decode(base)?;
    let mut rows = BTreeMap::new();
    for row in document.fades {
        if row.sources.len() > hex_project::MAX_RETAINED_AUDIO_DEPTH
            || row.curve.smooth == [false; 2]
            || rows
                .insert((row.clip, row.sources, row.slot), row.curve)
                .is_some()
        {
            return Err(StorageError::Invalid(
                "invalid or duplicate s-curve fade row",
            ));
        }
    }
    let mut snapshot = project.snapshot();
    visit(
        &project,
        &mut snapshot,
        |clip, sources, slot, fade| match rows.remove(&(clip, sources.to_vec(), slot)) {
            Some(curve) => curve.restore(fade),
            None => Ok(fade),
        },
    )?;
    if !rows.is_empty() {
        return Err(StorageError::Invalid("s-curve references a missing fade"));
    }
    Project::from_snapshot(snapshot).map_err(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{AudioEventEdit, Command, Created, Editor};
    use serde_json::Value;

    #[test]
    fn schema113_preserves_historical_gains_and_round_trips_nested_fades() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, track, asset } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 101,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let linear = ClipFade::new(101, 0, 41, 31).unwrap();
        let set = |fade| Command::SetClipFadeEnvelope { clip, fade };
        editor.execute(set(linear)).unwrap();
        let prior = crate::encode(editor.project()).unwrap();
        let historical = crate::decode(&prior).unwrap();
        assert_eq!(crate::encode(&historical).unwrap(), prior);
        let legacy = historical.clip(clip).unwrap().retained_fade();
        assert_eq!(legacy, linear);
        assert_eq!(
            [legacy.gain_at(10), legacy.gain_at(20), legacy.gain_at(30)],
            [0.25, 0.5, 0.75]
        );

        editor
            .execute(set(linear.with_curve_laws([
                ClipFade::DEFAULT_S_CURVE,
                FadeCurveLaw::Linear,
            ])))
            .unwrap();
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 61 },
            })
            .unwrap();
        let event_shape = ClipFade::new(101, 0, 41, 31)
            .unwrap()
            .with_curve_laws([FadeCurveLaw::Linear, ClipFade::DEFAULT_S_CURVE]);
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::SetFade {
                    event: 1,
                    fade: Some(AudioEventFade::new(0, event_shape).unwrap()),
                },
            })
            .unwrap();
        let Created::Clip(other) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 120,
                duration: 101,
                source_offset: 0,
                source_length: 101,
                looping: false,
            })
            .unwrap()
        else {
            panic!("place clip")
        };
        editor
            .execute(Command::GroupClips(vec![clip, other]))
            .unwrap();
        let expected = editor.project();
        let bytes = crate::encode(expected).unwrap();
        assert_eq!(
            crate::schema_version_for(expected),
            S_CURVE_FADE_SCHEMA_VERSION
        );
        let decoded = crate::decode(&bytes).unwrap();
        assert_eq!(decoded.snapshot(), expected.snapshot());
        assert_eq!(crate::encode(&decoded).unwrap(), bytes);
        assert!(crate::encode_schema22(expected).is_err());

        let folder = std::env::temp_dir().join(format!("hex-s-curve-{}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        let old_path = folder.join("linear.hex");
        std::fs::write(&old_path, &prior).unwrap();
        let (_, token) = crate::load_with_token(&old_path).unwrap();
        assert!(token.requires_save_confirmation(expected).unwrap());
        assert_eq!(std::fs::read(&old_path).unwrap(), prior);
        let path = folder.join("curves.hex");
        let saved = crate::save_new(&path, expected).and_then(|()| crate::load(&path));
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(saved.unwrap().snapshot(), expected.snapshot());

        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        let rows = value["fades"].as_array_mut().unwrap();
        assert_eq!(rows.len(), 3);
        rows.push(rows[0].clone());
        assert!(crate::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        let rows = value["fades"].as_array_mut().unwrap();
        rows.pop();
        rows[0]["clip"] = u64::MAX.into();
        assert!(crate::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
