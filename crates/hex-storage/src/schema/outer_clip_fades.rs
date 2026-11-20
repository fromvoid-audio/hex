//! Schema122 stores destination fade shape and clock context separately from
//! the prior codec's processing roots. The nested predecessor is a carrier
//! with neutral root fades at inventoried addresses. Only the fully restored
//! and validated project is returned. Schemas through121 keep their authored
//! root law. Earlier documents lack unwrapped repeat history, so no old fade
//! is reinterpreted as Outer. The regular incompatible overwrite fence applies.
use super::s_curve_fades::Curve;
use crate::{OUTER_CLIP_FADE_SCHEMA_VERSION, Project, StorageError, audio_edit::Projection};
use hex_project::{
    AliasContentSnapshot, AudioClipContent, AudioClipDomain, AudioClipFadeClock, AudioDomainClock,
    AudioFadeProjection, Clip, ClipFade, ClipSnapshot, MAX_CLIPS, MAX_RETAINED_AUDIO_DEPTH,
    Snapshot, TempoMap,
};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::value::RawValue;
use std::{collections::BTreeMap, sync::Arc};

const MAX_ROWS: usize = 2 * MAX_CLIPS;
type Address = (u64, Vec<u64>);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<'a> {
    format: &'a str,
    version: u32,
    #[serde(borrow)]
    base: &'a RawValue,
    #[serde(deserialize_with = "read_rows")]
    fades: Vec<Row>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    #[serde(deserialize_with = "read_path")]
    sources: Vec<u64>,
    fade: Fade,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fade {
    span: u64,
    offset: u64,
    fade_in: u64,
    fade_out: u64,
    prefix: u64,
    curve: Curve,
    #[serde(deserialize_with = "read_author")]
    author: Option<Projection>,
}

impl Fade {
    fn capture(domain: &AudioClipDomain) -> Self {
        let fade = domain.fade;
        Self {
            span: fade.span(),
            offset: fade.offset(),
            fade_in: fade.fade_in(),
            fade_out: fade.fade_out(),
            prefix: domain.fade_start,
            curve: Curve::capture(fade),
            author: domain.fade_projection.as_ref().map(Projection::from),
        }
    }

    fn install(self, domain: &mut AudioClipDomain) -> Result<(), StorageError> {
        domain.fade = self.curve.restore(
            ClipFade::new(self.span, self.offset, self.fade_in, self.fade_out).map_err(invalid)?,
        )?;
        domain.fade_start = self.prefix;
        domain.fade_projection = self.author.map(AudioFadeProjection::try_from).transpose()?;
        domain.fade_clock = AudioClipFadeClock::Outer;
        Ok(())
    }
}

fn read_author<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Projection>, D::Error> {
    Option::deserialize(d)
}
fn read_rows<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    bounded(d, MAX_ROWS)
}
fn read_path<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    bounded(d, MAX_RETAINED_AUDIO_DEPTH)
}
fn bounded<'de, D, T>(d: D, limit: usize) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Items<T>(usize, std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> de::Visitor<'de> for Items<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded outer fade inventory")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element()? {
                if items.len() == self.0 {
                    return Err(de::Error::custom("outer fade inventory limit exceeded"));
                }
                items.push(item);
            }
            Ok(items)
        }
    }
    d.deserialize_seq(Items(limit, std::marker::PhantomData))
}

fn invalid(error: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(error.0)
}

fn outer(clip: &Clip) -> bool {
    clip.audio_edit()
        .is_some_and(|edit| edit.domain.fade_clock == AudioClipFadeClock::Outer)
}

pub(crate) fn contains(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(|clip| outer(clip) || clip.retained_audio_roots().any(|root| outer(&root.clip)))
}

fn carrier_author(domain: &AudioClipDomain, tempo: &TempoMap) -> Option<AudioFadeProjection> {
    matches!(domain.clock, AudioDomainClock::Beats(_)).then(|| AudioFadeProjection {
        tempo: tempo.clone(),
        origin_beat_raw: 0,
        local_start_raw: 0,
    })
}

struct Inventory {
    fades: BTreeMap<Address, Fade>,
    restoring: bool,
}

impl Inventory {
    fn visit(
        &mut self,
        project: &Project,
        clip: &mut ClipSnapshot,
        tempo: &TempoMap,
        owner: u64,
        sources: &mut Vec<u64>,
    ) -> Result<bool, StorageError> {
        let Some(edit) = &mut clip.edit else {
            return Ok(false);
        };
        let domain = Arc::make_mut(&mut edit.domain);
        let mut changed = false;
        if self.restoring {
            if let Some(fade) = self.fades.remove(&(owner, sources.clone())) {
                if domain.fade_clock != AudioClipFadeClock::LegacyRoot
                    || domain.fade != ClipFade::neutral(clip.duration).map_err(invalid)?
                    || domain.fade_start != 0
                    || domain.fade_projection != carrier_author(domain, tempo)
                {
                    return Err(StorageError::Invalid(
                        "outer fade carrier has a conflicting root fade",
                    ));
                }
                fade.install(domain)?;
                changed = true;
            }
        } else if domain.fade_clock == AudioClipFadeClock::Outer {
            if self.fades.len() >= MAX_ROWS {
                return Err(StorageError::Invalid("outer fade inventory limit exceeded"));
            }
            self.fades
                .insert((owner, sources.clone()), Fade::capture(domain));
            domain.fade = ClipFade::neutral(clip.duration).map_err(invalid)?;
            domain.fade_start = 0;
            domain.fade_projection = carrier_author(domain, tempo);
            domain.fade_clock = AudioClipFadeClock::LegacyRoot;
            changed = true;
        }
        if let Some(events) = &mut domain.events
            && !events.sources.is_empty()
        {
            for root in &mut Arc::make_mut(events).sources {
                let mut snapshot = root.clip.retained_snapshot();
                sources.push(root.id);
                let replaced = self.visit(project, &mut snapshot, &root.tempo, owner, sources)?;
                sources.pop();
                if replaced {
                    *root = project
                        .restore_retained_audio_root(root.id, snapshot, Arc::clone(&root.tempo))
                        .map_err(invalid)?;
                    changed = true;
                }
            }
        }
        Ok(changed)
    }

    fn snapshot(&mut self, project: &Project) -> Result<Snapshot, StorageError> {
        let mut snapshot = project.snapshot();
        let members: BTreeMap<_, _> = snapshot
            .aliases
            .members
            .iter()
            .map(|member| (member.clip, member.source))
            .collect();
        let mut updated_sources: BTreeMap<u64, Arc<AudioClipContent>> = BTreeMap::new();
        for track in &mut snapshot.tracks {
            if let Ok(clips) = track.audio_clips_mut() {
                for clip in clips {
                    if self.visit(project, clip, project.tempo_map(), clip.id, &mut Vec::new())?
                        && let Some(source) = members.get(&clip.id)
                        && let Some(edit) = &clip.edit
                    {
                        updated_sources.insert(*source, Arc::clone(&edit.domain.content));
                    }
                }
            }
        }
        // Local fades live outside the alias source. A changed descendant,
        // however, belongs to the shared content. Project validation checks
        // every member against this restored source before rebinding.
        for source in &mut snapshot.aliases.sources {
            if let AliasContentSnapshot::Audio { content, .. } = &mut source.content
                && let Some(updated) = updated_sources.remove(&source.id)
            {
                *content = updated;
            }
        }
        Ok(snapshot)
    }
}

pub(crate) fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    let mut inventory = Inventory {
        fades: BTreeMap::new(),
        restoring: false,
    };
    let base = Project::from_snapshot(inventory.snapshot(project)?).map_err(invalid)?;
    if inventory.fades.is_empty() || contains(&base) {
        return Err(StorageError::Invalid("incomplete outer fade inventory"));
    }
    let base = crate::encode(&base)?;
    crate::encode_document(&Document {
        format: "hex-project",
        version: OUTER_CLIP_FADE_SCHEMA_VERSION,
        base: serde_json::from_slice(&base)?,
        fades: inventory
            .fades
            .into_iter()
            .map(|((clip, sources), fade)| Row {
                clip,
                sources,
                fade,
            })
            .collect(),
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
    if document.version != OUTER_CLIP_FADE_SCHEMA_VERSION
        || header.version >= document.version
        || document.fades.is_empty()
    {
        return Err(StorageError::Invalid("invalid outer fade envelope"));
    }
    let mut inventory = Inventory {
        fades: BTreeMap::new(),
        restoring: true,
    };
    for row in document.fades {
        if inventory
            .fades
            .insert((row.clip, row.sources), row.fade)
            .is_some()
        {
            return Err(StorageError::Invalid("duplicate outer fade address"));
        }
    }
    let base = crate::decode(base)?;
    let snapshot = inventory.snapshot(&base)?;
    if !inventory.fades.is_empty() {
        return Err(StorageError::Invalid(
            "outer fade address has no retained audio domain",
        ));
    }
    Project::from_snapshot(snapshot).map_err(invalid)
}
