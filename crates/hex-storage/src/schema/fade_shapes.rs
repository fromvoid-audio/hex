//! Schema29 layers bounded fade shapes over the strict schema28 event envelope.
use super::*;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub(crate) struct DocumentV29<'a> {
    #[serde(flatten)]
    base: super::audio_events::Document<'a>,
    fade_shapes: Vec<FadeShape>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FadeShape {
    clip: u64,
    in_start: u64,
    out_end_offset: u64,
    in_curve: i16,
    out_curve: i16,
}
#[derive(Deserialize)]
struct BoundedShapes(#[serde(deserialize_with = "bounded_shapes")] Vec<FadeShape>);
fn bounded_shapes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<FadeShape>, D::Error> {
    struct Shapes;
    impl<'de> serde::de::Visitor<'de> for Shapes {
        type Value = Vec<FadeShape>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded arrangement fade shapes")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut shapes = Vec::new();
            while let Some(shape) = seq.next_element()? {
                if shapes.len() == hex_project::MAX_CLIPS {
                    return Err(serde::de::Error::custom("fade shape limit reached"));
                }
                shapes.push(shape);
            }
            Ok(shapes)
        }
    }
    d.deserialize_seq(Shapes)
}
fn apply_shapes(project: Project, shapes: Vec<FadeShape>) -> Result<Project, StorageError> {
    if shapes.len() > hex_project::MAX_CLIPS {
        return Err(StorageError::Invalid("fade shape limit reached"));
    }
    let mut shapes_by_id = BTreeMap::new();
    for shape in shapes {
        if shapes_by_id.insert(shape.clip, shape).is_some() {
            return Err(StorageError::Invalid("duplicate fade shape"));
        }
    }
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut track.content else {
            continue;
        };
        for clip in audio_clips {
            let shape = shapes_by_id
                .remove(&clip.id)
                .ok_or(StorageError::Invalid("fade shape is missing"))?;
            let target = if let Some(edit) = &mut clip.edit {
                &mut std::sync::Arc::make_mut(&mut edit.domain).fade
            } else {
                &mut clip.fade
            };
            *target = target
                .with_shape(
                    shape.in_start,
                    shape.out_end_offset,
                    shape.in_curve,
                    shape.out_curve,
                )
                .map_err(|e| StorageError::Invalid(e.0))?;
        }
    }
    if !shapes_by_id.is_empty() {
        return Err(StorageError::Invalid(
            "fade shape references a non-arrangement audio clip",
        ));
    }
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}
impl<'a> DocumentV29<'a> {
    pub(crate) fn from_project(project: &'a Project) -> Result<Self, StorageError> {
        super::device_assets::refuse_legacy(project)?;
        Self::base(project)
    }
    pub(super) fn for_asset_envelope(project: &'a Project) -> Result<Self, StorageError> {
        let mut document = Self::base(project)?;
        document.base.base.version = 30;
        Ok(document)
    }
    fn base(project: &'a Project) -> Result<Self, StorageError> {
        let mut base = super::audio_events::encode(project)?;
        base.base.version = 29;
        let fade_shapes = project
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .map(|clip| {
                let fade = clip.retained_fade();
                FadeShape {
                    clip: clip.id().value(),
                    in_start: fade.in_start(),
                    out_end_offset: fade.out_end_offset(),
                    in_curve: fade.in_curve(),
                    out_curve: fade.out_curve(),
                }
            })
            .collect();
        Ok(Self { base, fade_shapes })
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        // Validate the original token stream before Value adaptation can discard
        // duplicate keys, exactly as the underlying strict event codec does.
        let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
        let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
        let object = value
            .as_object_mut()
            .ok_or(StorageError::Invalid("invalid fade envelope"))?;
        if object.get("version").and_then(|v| v.as_u64()) != Some(29) {
            return Err(StorageError::Invalid("invalid fade envelope version"));
        }
        let shapes: BoundedShapes = serde_json::from_value(
            object
                .remove("fade_shapes")
                .ok_or(StorageError::Invalid("fade shapes are missing"))?,
        )?;
        object.insert("version".into(), 28.into());
        let base = super::audio_events::decode(&serde_json::to_vec(&value)?)?;
        apply_shapes(base, shapes.0)
    }
}
