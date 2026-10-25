//! Required current-envelope classification and expected original PCM content.
//! These metadata claims never replace reconstruction through native ingress.
use super::*;
use hex_project::OriginalSourceFacts;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    asset: u64,
    #[serde(deserialize_with = "required_original")]
    original: Option<Facts>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Facts {
    law_version: u32,
    content_digest: [u8; 32],
}
fn required_original<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Facts>, D::Error> {
    Option::deserialize(d)
}
#[derive(Serialize, Deserialize)]
pub(super) struct Manifest(#[serde(deserialize_with = "bounded_rows")] Vec<Row>);
fn bounded_rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    struct Rows;
    impl<'de> serde::de::Visitor<'de> for Rows {
        type Value = Vec<Row>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("complete bounded asset source inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<Row>, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == hex_project::MAX_ASSETS {
                    return Err(serde::de::Error::custom(
                        "asset source inventory limit reached",
                    ));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows)
}
impl Manifest {
    pub(super) fn from_project(project: &Project) -> Self {
        Self(
            project
                .assets()
                .map(|asset| Row {
                    asset: asset.id().value(),
                    original: asset.original_source().map(|facts| Facts {
                        law_version: facts.law_version(),
                        content_digest: facts.content_digest(),
                    }),
                })
                .collect(),
        )
    }
    pub(super) fn apply(self, snapshot: &mut hex_project::Snapshot) -> Result<(), StorageError> {
        let mut rows = BTreeMap::new();
        for row in self.0 {
            let facts = row
                .original
                .map(|f| {
                    OriginalSourceFacts::new(f.law_version, f.content_digest)
                        .map_err(|e| StorageError::Invalid(e.0))
                })
                .transpose()?;
            if rows.insert(row.asset, facts).is_some() {
                return Err(StorageError::Invalid(
                    "duplicate asset source inventory entry",
                ));
            }
        }
        for asset in &mut snapshot.assets {
            asset.original_source = rows.remove(&asset.id).ok_or(StorageError::Invalid(
                "asset source inventory entry is missing",
            ))?;
        }
        if !rows.is_empty() {
            return Err(StorageError::Invalid(
                "asset source inventory references missing media",
            ));
        }
        Ok(())
    }
}
