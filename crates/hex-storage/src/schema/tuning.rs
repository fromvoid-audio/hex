//! Schema30 adds required project tuning to the complete strict schema29 envelope.
use super::{Project, StorageError};
use hex_tuning::{Audition, ProjectTuning, Scale, Waveform};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(crate) struct DocumentV30<'a> {
    #[serde(flatten)]
    base: super::fade_shapes::DocumentV29<'a>,
    tuning: Tuning,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tuning {
    enabled: bool,
    scale: ScaleDto,
    audition: AuditionDto,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScaleDto {
    count: usize,
    #[serde(deserialize_with = "bounded_cents")]
    cents: Vec<f32>,
    root_key: u8,
    root_hz: f64,
    root_degree: usize,
    period_cents: f32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditionDto {
    waveform: WaveformDto,
    level: f32,
    attack_ms: f32,
    release_ms: f32,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WaveformDto {
    Triangle,
    Square,
    Saw,
}
fn bounded_cents<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<f32>, D::Error> {
    struct Cents;
    impl<'de> serde::de::Visitor<'de> for Cents {
        type Value = Vec<f32>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("exactly 48 tuning degree values")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == 48 {
                    return Err(serde::de::Error::custom("tuning degree limit reached"));
                }
                values.push(value);
            }
            if values.len() != 48 {
                return Err(serde::de::Error::custom("tuning requires 48 degree values"));
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Cents)
}
impl From<ProjectTuning> for Tuning {
    fn from(tuning: ProjectTuning) -> Self {
        let scale = tuning.scale();
        let audition = tuning.audition();
        Self {
            enabled: tuning.enabled(),
            scale: ScaleDto {
                count: scale.count(),
                cents: scale.cents().to_vec(),
                root_key: scale.root_key(),
                root_hz: scale.root_hz(),
                root_degree: scale.root_degree(),
                period_cents: scale.period_cents(),
            },
            audition: AuditionDto {
                waveform: match audition.waveform() {
                    Waveform::Triangle => WaveformDto::Triangle,
                    Waveform::Square => WaveformDto::Square,
                    Waveform::Saw => WaveformDto::Saw,
                },
                level: audition.level(),
                attack_ms: audition.attack_ms(),
                release_ms: audition.release_ms(),
            },
        }
    }
}
impl Tuning {
    fn into_tuning(self) -> Result<ProjectTuning, StorageError> {
        let cents = self
            .scale
            .cents
            .try_into()
            .map_err(|_| StorageError::Invalid("tuning requires 48 degree values"))?;
        let scale = Scale::new(
            self.scale.count,
            cents,
            self.scale.root_key,
            self.scale.root_hz,
            self.scale.root_degree,
            self.scale.period_cents,
        )
        .map_err(|e| StorageError::Invalid(e.0))?;
        let waveform = match self.audition.waveform {
            WaveformDto::Triangle => Waveform::Triangle,
            WaveformDto::Square => Waveform::Square,
            WaveformDto::Saw => Waveform::Saw,
        };
        let audition = Audition::new(
            waveform,
            self.audition.level,
            self.audition.attack_ms,
            self.audition.release_ms,
        )
        .map_err(|e| StorageError::Invalid(e.0))?;
        ProjectTuning::new(self.enabled, scale, audition).map_err(|e| StorageError::Invalid(e.0))
    }
}
impl<'a> DocumentV30<'a> {
    pub(crate) fn from_project(project: &'a Project) -> Result<Self, StorageError> {
        let mut base = super::fade_shapes::DocumentV29::from_project(project)?;
        base.use_tuning_version();
        Ok(Self {
            base,
            tuning: project.tuning().into(),
        })
    }
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    // Preserve duplicate-field refusal before adapting the older strict envelope.
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid tuning envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(30) {
        return Err(StorageError::Invalid("invalid tuning envelope version"));
    }
    let tuning: Tuning = serde_json::from_value(
        object
            .remove("tuning")
            .ok_or(StorageError::Invalid("project tuning is missing"))?,
    )?;
    let tuning = tuning.into_tuning()?;
    object.insert("version".into(), 29.into());
    let base = super::fade_shapes::DocumentV29::decode(&serde_json::to_vec(&value)?)?;
    let mut snapshot = base.snapshot();
    snapshot.tuning = tuning;
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}
