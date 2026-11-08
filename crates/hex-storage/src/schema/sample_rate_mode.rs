//! Schema76 extends the complete schema75 layout with a required
//! `sample_rate_mode` field. Only Follow Interface needs the new layout.
//! Earlier versions migrate as Fixed at their authored rate.
use crate::{
    Project, SAMPLE_RATE_MODE_SCHEMA_VERSION, StorageError, VST3_STATE_SIZE_SCHEMA_VERSION,
};
use hex_project::SampleRateMode;
use serde_json::Value;

pub(crate) fn contains(project: &Project) -> bool {
    project.sample_rate_mode() == SampleRateMode::FollowInterface
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut value = super::group_output_mute::encode(project)?;
    value["version"] = SAMPLE_RATE_MODE_SCHEMA_VERSION.into();
    value["sample_rate_mode"] = "follow_interface".into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    let mode = value
        .as_object_mut()
        .and_then(|map| map.remove("sample_rate_mode"));
    if mode.as_ref().and_then(Value::as_str) != Some("follow_interface") {
        return Err(StorageError::Invalid("invalid project sample rate mode"));
    }
    value["version"] = VST3_STATE_SIZE_SCHEMA_VERSION.into();
    super::group_output_mute::decode_value(value, VST3_STATE_SIZE_SCHEMA_VERSION)
        .map(|project| project.with_sample_rate_mode(SampleRateMode::FollowInterface))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_project_keeps_its_clock_and_follow_mode_round_trips() {
        let bytes = include_bytes!("../../tests/fixtures/track-name-origin/schema62.json");
        let fixed = crate::decode(bytes).unwrap();
        assert_eq!(fixed.sample_rate_mode(), SampleRateMode::Fixed);
        assert!(crate::schema_version_for(&fixed) < SAMPLE_RATE_MODE_SCHEMA_VERSION);
        let rate = fixed.sample_rate();
        let follow = fixed.with_sample_rate_mode(SampleRateMode::FollowInterface);
        let encoded = crate::encode(&follow).unwrap();
        let reopened = crate::decode(&encoded).unwrap();
        assert_eq!(reopened.sample_rate(), rate);
        assert_eq!(reopened, follow);
        assert!(crate::encode_schema22(&follow).is_err());
        assert!(crate::encode_schema20(&follow).is_err());

        let mut malformed: Value = serde_json::from_slice(&encoded).unwrap();
        for invalid in [Value::Null, "fixed".into(), 1.into()] {
            malformed["sample_rate_mode"] = invalid;
            assert!(crate::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
        }
        malformed
            .as_object_mut()
            .unwrap()
            .remove("sample_rate_mode");
        assert!(crate::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
    }
}
