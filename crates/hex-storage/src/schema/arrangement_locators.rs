//! Schema114 stores authored locators over a complete prior project document.
//! Earlier formats acquire an empty inventory. Projects without locators keep
//! their compatible schema, and the ordinary overwrite guard handles upgrades.
use crate::{ARRANGEMENT_LOCATOR_SCHEMA_VERSION, Project, StorageError};
use hex_project::{ArrangementLocatorSnapshot, MAX_ARRANGEMENT_LOCATORS};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::value::RawValue;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<'a, Base> {
    format: &'a str,
    version: u32,
    base: Base,
    #[serde(deserialize_with = "bounded_locators")]
    locators: Vec<Row>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    id: u64,
    frame: u64,
    #[serde(deserialize_with = "required_name")]
    name: Option<String>,
}

fn required_name<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::deserialize(d)
}

fn bounded_locators<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    struct Rows;
    impl<'de> de::Visitor<'de> for Rows {
        type Value = Vec<Row>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded arrangement locator inventory")
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<Row>, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == MAX_ARRANGEMENT_LOCATORS {
                    return Err(de::Error::custom("arrangement locator limit reached"));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows)
}

pub(crate) fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    let mut snapshot = project.snapshot();
    let locators = std::mem::take(&mut snapshot.arrangement_locators)
        .into_iter()
        .map(|ArrangementLocatorSnapshot { id, frame, name }| Row { id, frame, name })
        .collect();
    let prior = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    if super::s_curve_fades::contains(&prior) {
        encode_over(super::s_curve_fades::document(&prior)?, locators)
    } else {
        encode_over(crate::encode_prior(&prior)?, locators)
    }
}

fn encode_over(base: impl Serialize, locators: Vec<Row>) -> Result<Vec<u8>, StorageError> {
    crate::encode_document(&Document {
        format: "hex-project",
        version: ARRANGEMENT_LOCATOR_SCHEMA_VERSION,
        base,
        locators,
    })
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.len() > crate::MAX_PROJECT_DOCUMENT_BYTES {
        return Err(StorageError::Invalid(
            "project exceeds the document size limit",
        ));
    }
    let document: Document<'_, &RawValue> = serde_json::from_slice(bytes)?;
    #[derive(Deserialize)]
    struct Header {
        version: u32,
    }
    let base = document.base.get().as_bytes();
    let header: Header = serde_json::from_slice(base)?;
    if header.version >= ARRANGEMENT_LOCATOR_SCHEMA_VERSION || document.locators.is_empty() {
        return Err(StorageError::Invalid(
            "invalid arrangement locator envelope",
        ));
    }
    let mut snapshot = crate::decode(base)?.snapshot();
    snapshot.arrangement_locators = document
        .locators
        .into_iter()
        .map(|Row { id, frame, name }| ArrangementLocatorSnapshot { id, frame, name })
        .collect();
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{ClipFade, Command, Created, Editor};
    use serde_json::Value;

    fn assert_prior_document_bytes(project: &Project, bytes: &[u8]) {
        let mut snapshot = project.snapshot();
        snapshot.arrangement_locators.clear();
        let prior = crate::encode(&Project::from_snapshot(snapshot).unwrap()).unwrap();
        let envelope: Document<'_, &RawValue> = serde_json::from_slice(bytes).unwrap();
        assert_eq!(envelope.base.get().as_bytes(), prior);
        assert_eq!(crate::encode_document(&envelope).unwrap(), bytes);
    }

    #[test]
    fn locators_preserve_prior_bytes_without_a_curve_envelope() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(Command::AddArrangementLocator { frame: 40 })
            .unwrap();
        let project = editor.project();
        let bytes = crate::encode(project).unwrap();
        assert_prior_document_bytes(project, &bytes);
        assert_eq!(crate::decode(&bytes).unwrap(), *project);
    }

    #[test]
    fn locators_round_trip_over_a_prior_s_curve_document() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "sample".into(),
                path: "sample.wav".into(),
                sample_rate: 48_000,
                frames: 101,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let fade = ClipFade::new(101, 0, 41, 41)
            .unwrap()
            .with_curve_laws([ClipFade::DEFAULT_S_CURVE; 2]);
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        let prior = crate::encode(editor.project()).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&prior).unwrap()["version"],
            113
        );
        let migrated = crate::decode(&prior).unwrap();
        assert!(migrated.arrangement_locators().is_empty());
        assert_eq!(migrated, *editor.project());
        assert_eq!(crate::encode(&migrated).unwrap(), prior);

        let Created::ArrangementLocator(locator) = editor
            .execute(Command::AddArrangementLocator { frame: 40 })
            .unwrap()
        else {
            panic!("locator")
        };
        editor
            .execute(Command::RenameArrangementLocator {
                locator,
                name: Some("Verse".into()),
            })
            .unwrap();
        editor
            .execute(Command::AddArrangementLocator { frame: 101 })
            .unwrap();
        let expected = editor.project();
        let bytes = crate::encode(expected).unwrap();
        assert_eq!(
            crate::schema_version_for(expected),
            ARRANGEMENT_LOCATOR_SCHEMA_VERSION
        );
        assert_eq!(crate::decode(&bytes).unwrap(), *expected);
        assert_prior_document_bytes(expected, &bytes);
        assert!(crate::encode_schema22(expected).is_err());

        let folder = std::env::temp_dir().join(format!("hex-locators-{}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("arrangement.hex");
        std::fs::write(&path, &prior).unwrap();
        let (_, token) = crate::load_with_token(&path).unwrap();
        assert!(token.requires_save_confirmation(expected).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), prior);
        let new_path = folder.join("locators.hex");
        let loaded = crate::save_new(&new_path, expected).and_then(|()| crate::load(&new_path));
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(loaded.unwrap(), *expected);

        let value: Value = serde_json::from_slice(&bytes).unwrap();
        for edit in [
            |v: &mut Value| v["version"] = (ARRANGEMENT_LOCATOR_SCHEMA_VERSION + 1).into(),
            |v: &mut Value| v["locators"][0]["id"] = 1.into(),
            |v: &mut Value| v["locators"][0]["frame"] = 101.into(),
            |v: &mut Value| v["locators"][0]["name"] = "".into(),
            |v: &mut Value| v["locators"][0]["frame"] = u64::MAX.into(),
            |v: &mut Value| v["locators"] = serde_json::json!([]),
            |v: &mut Value| {
                v["locators"][0].as_object_mut().unwrap().remove("name");
            },
        ] {
            let mut malformed = value.clone();
            edit(&mut malformed);
            assert!(crate::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
        }
    }
}
