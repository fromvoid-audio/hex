//! Bounded strict current-format structural song markers.
use super::*;
use hex_project::{ArrangementSectionSnapshot, ElementColor, MAX_ARRANGEMENT_SECTIONS};
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct Manifest(#[serde(deserialize_with = "bounded")] Vec<Section>);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    id: u64,
    name: String,
    color: String,
    start_raw: u64,
    end_raw: u64,
}
impl Manifest {
    pub(super) fn from_project(project: &Project) -> Self {
        Self(
            project
                .arrangement_sections()
                .iter()
                .map(|s| Section {
                    id: s.id().value(),
                    name: s.name().to_owned(),
                    color: s.color().key().to_owned(),
                    start_raw: s.start().raw(),
                    end_raw: s.end().raw(),
                })
                .collect(),
        )
    }
    pub(super) fn into_sections(self) -> Result<Vec<ArrangementSectionSnapshot>, StorageError> {
        self.0
            .into_iter()
            .map(|s| {
                Ok(ArrangementSectionSnapshot {
                    id: s.id,
                    name: s.name,
                    color: ElementColor::from_key(&s.color)
                        .map_err(|e| StorageError::Invalid(e.0))?,
                    start: BeatTime::from_raw(s.start_raw),
                    end: BeatTime::from_raw(s.end_raw),
                })
            })
            .collect()
    }
}
fn bounded<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Section>, D::Error> {
    struct Sections;
    impl<'de> serde::de::Visitor<'de> for Sections {
        type Value = Vec<Section>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded arrangement sections")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut sections = Vec::new();
            while let Some(section) = seq.next_element()? {
                if sections.len() == MAX_ARRANGEMENT_SECTIONS {
                    return Err(serde::de::Error::custom(
                        "arrangement section limit reached",
                    ));
                }
                sections.push(section);
            }
            Ok(sections)
        }
    }
    d.deserialize_seq(Sections)
}
