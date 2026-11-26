//! Neutral library identity and validated tag command/query boundary (#7420).
//!
//! Keeps stable item identity (`LibraryItemId`), monotonic content revision
//! (`LibraryRevision`), and filesystem location (`PathBuf`) distinct so Hex and
//! external library adapters share one narrow, validated contract without
//! competing tag databases or UI-owned metadata state.
use crate::catalog::{
    AudioHeaderMetadata, Fingerprint, MAX_TAG_BYTES, SampleCatalog, normalize_label,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

#[cfg(test)]
mod tests;

/// Maximum items returned per page query.
pub const MAX_PAGE_LIMIT: usize = 512;
/// Maximum distinct tags in a bounded hierarchical taxonomy.
pub const MAX_TAXONOMY_TAGS: usize = 1_024;
/// Maximum nesting depth of a tag hierarchy (root = 0).
pub const MAX_TAXONOMY_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LibraryItemId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TagId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LibraryRevision(pub u64);

impl LibraryRevision {
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibrarySource {
    pub id: SourceId,
    pub root: PathBuf,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagDefinition {
    pub id: TagId,
    pub name: String,
    pub parent: Option<TagId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryItemMeta {
    pub id: LibraryItemId,
    pub source_id: Option<SourceId>,
    pub path: PathBuf,
    pub name: String,
    pub revision: LibraryRevision,
    pub fingerprint: Option<Fingerprint>,
    pub audio: Option<AudioHeaderMetadata>,
    pub tags: Vec<TagId>,
    pub missing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    pub offset: usize,
    pub limit: usize,
}

impl PageRequest {
    pub fn new(offset: usize, limit: usize) -> Self {
        Self {
            offset,
            limit: limit.clamp(1, MAX_PAGE_LIMIT),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub offset: usize,
    pub revision: LibraryRevision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryCommand {
    CreateTag {
        name: String,
        parent: Option<TagId>,
    },
    RenameTag {
        tag: TagId,
        name: String,
    },
    MergeTag {
        source: TagId,
        target: TagId,
    },
    DeleteTag {
        tag: TagId,
    },
    AssignTag {
        item: LibraryItemId,
        tag: TagId,
    },
    RemoveTag {
        item: LibraryItemId,
        tag: TagId,
    },
    BulkAssignTag {
        items: Vec<LibraryItemId>,
        tag: TagId,
    },
    BulkRemoveTag {
        items: Vec<LibraryItemId>,
        tag: TagId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryCommandEnvelope {
    pub expected_revision: LibraryRevision,
    pub command: LibraryCommand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LibraryCommandReceipt {
    pub committed_revision: LibraryRevision,
    pub created_tag: Option<TagId>,
    pub changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryBoundaryError {
    StaleRevision {
        expected: LibraryRevision,
        current: LibraryRevision,
    },
    UnknownItem(LibraryItemId),
    UnknownTag(TagId),
    InvalidLabel(&'static str),
    DuplicateTag(String),
    CycleDetected,
    DepthLimitExceeded,
    LimitReached(&'static str),
    Storage(String),
}

impl std::fmt::Display for LibraryBoundaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleRevision { expected, current } => write!(
                f,
                "stale library revision: expected {}, current {}",
                expected.0, current.0
            ),
            Self::UnknownItem(id) => write!(f, "unknown library item {}", id.0),
            Self::UnknownTag(id) => write!(f, "unknown library tag {}", id.0),
            Self::InvalidLabel(msg) => f.write_str(msg),
            Self::DuplicateTag(name) => write!(f, "tag '{name}' already exists"),
            Self::CycleDetected => f.write_str("tag hierarchy cannot contain a cycle"),
            Self::DepthLimitExceeded => f.write_str("tag hierarchy depth limit reached"),
            Self::LimitReached(msg) => f.write_str(msg),
            Self::Storage(msg) => write!(f, "library storage failed: {msg}"),
        }
    }
}

impl std::error::Error for LibraryBoundaryError {}

/// Neutral query and validated mutation boundary over a sample/tag library.
pub trait LibraryBoundary {
    fn revision(&self) -> LibraryRevision;
    fn list_sources(&self) -> Vec<LibrarySource>;
    fn list_tags(&self) -> Vec<TagDefinition>;
    fn descendant_tags(&self, tag: TagId) -> Vec<TagId>;
    fn page_items(&self, source: Option<SourceId>, page: PageRequest) -> Page<LibraryItemMeta>;
    fn execute(
        &mut self,
        envelope: LibraryCommandEnvelope,
    ) -> Result<LibraryCommandReceipt, LibraryBoundaryError>;
}

/// Concrete neutral-boundary adapter reusing Hex's canonical [`SampleCatalog`]
/// without creating a second competing database.
#[derive(Clone, Debug)]
pub struct CatalogLibraryAdapter {
    catalog: SampleCatalog,
    backing_path: Option<PathBuf>,
    revision: LibraryRevision,
    next_id: u64,
    sources: Vec<LibrarySource>,
    path_to_item: HashMap<PathBuf, LibraryItemId>,
    item_to_path: BTreeMap<LibraryItemId, PathBuf>,
    item_revisions: HashMap<LibraryItemId, LibraryRevision>,
    tags: BTreeMap<TagId, TagDefinition>,
    tag_name_to_id: BTreeMap<String, TagId>,
}

impl CatalogLibraryAdapter {
    /// Construct a boundary adapter around an existing [`SampleCatalog`] and
    /// configured source roots, assigning stable neutral IDs to every known
    /// source, sample entry, and existing catalog tag.
    pub fn from_catalog(
        catalog: SampleCatalog,
        roots: &[PathBuf],
        backing_path: Option<PathBuf>,
    ) -> Self {
        let mut adapter = Self {
            catalog,
            backing_path,
            revision: LibraryRevision(1),
            next_id: 1,
            sources: Vec::new(),
            path_to_item: HashMap::new(),
            item_to_path: BTreeMap::new(),
            item_revisions: HashMap::new(),
            tags: BTreeMap::new(),
            tag_name_to_id: BTreeMap::new(),
        };
        for root in roots {
            let id = SourceId(adapter.allocate_id());
            let label = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string());
            adapter.sources.push(LibrarySource {
                id,
                root: root.clone(),
                label,
            });
        }
        let paths: Vec<PathBuf> = adapter
            .catalog
            .entries()
            .iter()
            .map(|e| e.path.clone())
            .collect();
        for path in paths {
            let id = LibraryItemId(adapter.allocate_id());
            adapter.path_to_item.insert(path.clone(), id);
            adapter.item_to_path.insert(id, path);
            adapter.item_revisions.insert(id, adapter.revision);
        }
        let existing_tags = adapter.catalog.all_tags();
        for name in existing_tags {
            let id = TagId(adapter.allocate_id());
            adapter.tag_name_to_id.insert(name.clone(), id);
            adapter.tags.insert(
                id,
                TagDefinition {
                    id,
                    name,
                    parent: None,
                },
            );
        }
        adapter
    }

    pub fn catalog(&self) -> &SampleCatalog {
        &self.catalog
    }

    pub fn item_id_for_path(&self, path: &Path) -> Option<LibraryItemId> {
        self.path_to_item.get(path).copied()
    }

    pub fn tag_id_for_name(&self, name: &str) -> Option<TagId> {
        let normalized = normalize_label(name).ok()?;
        self.tag_name_to_id.get(&normalized).copied()
    }

    /// Relocate a source root while preserving every item's stable [`LibraryItemId`],
    /// tags, and records.
    pub fn relocate_source(
        &mut self,
        source_id: SourceId,
        new_root: PathBuf,
    ) -> Result<LibraryRevision, LibraryBoundaryError> {
        let Some(source) = self.sources.iter().find(|s| s.id == source_id) else {
            return Err(LibraryBoundaryError::InvalidLabel(
                "library source does not exist",
            ));
        };
        let old_root = source.root.clone();
        if old_root == new_root || !new_root.is_absolute() {
            return Err(LibraryBoundaryError::InvalidLabel(
                "relocated source root must be a distinct absolute path",
            ));
        }
        let snapshot_catalog = self.catalog.clone();
        let snapshot_sources = self.sources.clone();
        let snapshot_path_to_item = self.path_to_item.clone();
        let snapshot_item_to_path = self.item_to_path.clone();
        let snapshot_item_revisions = self.item_revisions.clone();
        let next_rev = self.revision.next();

        self.catalog.relocate_root(&old_root, &new_root);
        if let Some(src) = self.sources.iter_mut().find(|s| s.id == source_id) {
            src.root = new_root.clone();
            src.label = new_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| new_root.display().to_string());
        }
        let mut updated_p2i = HashMap::with_capacity(self.path_to_item.len());
        let mut updated_i2p = BTreeMap::new();
        for (item_id, old_path) in &self.item_to_path {
            let next_path = old_path
                .strip_prefix(&old_root)
                .map(|rel| new_root.join(rel))
                .unwrap_or_else(|_| old_path.clone());
            if next_path != *old_path {
                self.item_revisions.insert(*item_id, next_rev);
            }
            updated_p2i.insert(next_path.clone(), *item_id);
            updated_i2p.insert(*item_id, next_path);
        }
        self.path_to_item = updated_p2i;
        self.item_to_path = updated_i2p;

        if let Err(err) = self.persist_if_backed() {
            self.catalog = snapshot_catalog;
            self.sources = snapshot_sources;
            self.path_to_item = snapshot_path_to_item;
            self.item_to_path = snapshot_item_to_path;
            self.item_revisions = snapshot_item_revisions;
            return Err(err);
        }
        self.revision = next_rev;
        Ok(self.revision)
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn tag_depth(&self, mut parent: Option<TagId>) -> Result<usize, LibraryBoundaryError> {
        let mut depth = 0_usize;
        let mut visited = BTreeSet::new();
        while let Some(id) = parent {
            if !visited.insert(id) {
                return Err(LibraryBoundaryError::CycleDetected);
            }
            depth += 1;
            if depth > MAX_TAXONOMY_DEPTH {
                return Err(LibraryBoundaryError::DepthLimitExceeded);
            }
            let Some(def) = self.tags.get(&id) else {
                return Err(LibraryBoundaryError::UnknownTag(id));
            };
            parent = def.parent;
        }
        Ok(depth)
    }

    fn persist_if_backed(&self) -> Result<(), LibraryBoundaryError> {
        if let Some(path) = &self.backing_path {
            self.catalog
                .save(path)
                .map_err(|err| LibraryBoundaryError::Storage(err.to_string()))?;
        }
        Ok(())
    }

    fn source_for_path(&self, path: &Path) -> Option<SourceId> {
        self.sources
            .iter()
            .find(|source| path.starts_with(&source.root))
            .map(|source| source.id)
    }
}

impl LibraryBoundary for CatalogLibraryAdapter {
    fn revision(&self) -> LibraryRevision {
        self.revision
    }

    fn list_sources(&self) -> Vec<LibrarySource> {
        self.sources.clone()
    }

    fn list_tags(&self) -> Vec<TagDefinition> {
        self.tags.values().cloned().collect()
    }

    fn descendant_tags(&self, tag: TagId) -> Vec<TagId> {
        if !self.tags.contains_key(&tag) {
            return Vec::new();
        }
        let mut out = vec![tag];
        let mut frontier = vec![tag];
        let mut seen = BTreeSet::from([tag]);
        while let Some(current) = frontier.pop() {
            for def in self.tags.values() {
                if def.parent == Some(current) && seen.insert(def.id) {
                    out.push(def.id);
                    frontier.push(def.id);
                }
            }
        }
        out
    }

    fn page_items(&self, source: Option<SourceId>, page: PageRequest) -> Page<LibraryItemMeta> {
        let limit = page.limit.clamp(1, MAX_PAGE_LIMIT);
        let root_filter = source.and_then(|id| {
            self.sources
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.root.as_path())
        });
        if source.is_some() && root_filter.is_none() {
            return Page {
                items: Vec::new(),
                total: 0,
                offset: page.offset,
                revision: self.revision,
            };
        }
        let matching: Vec<_> = self
            .catalog
            .entries()
            .iter()
            .filter(|entry| root_filter.is_none_or(|root| entry.path.starts_with(root)))
            .collect();
        let total = matching.len();
        let items = matching
            .into_iter()
            .skip(page.offset)
            .take(limit)
            .filter_map(|entry| {
                let id = *self.path_to_item.get(&entry.path)?;
                let record = self.catalog.record(&entry.path);
                let tags = self
                    .catalog
                    .tags_for(&entry.path)
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(|n| self.tag_name_to_id.get(n).copied())
                            .collect()
                    })
                    .unwrap_or_default();
                Some(LibraryItemMeta {
                    id,
                    source_id: self.source_for_path(&entry.path),
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    revision: self
                        .item_revisions
                        .get(&id)
                        .copied()
                        .unwrap_or(self.revision),
                    fingerprint: record.and_then(|r| r.fingerprint),
                    audio: record.and_then(|r| r.audio),
                    tags,
                    missing: !entry.path.exists(),
                })
            })
            .collect();
        Page {
            items,
            total,
            offset: page.offset,
            revision: self.revision,
        }
    }

    fn execute(
        &mut self,
        envelope: LibraryCommandEnvelope,
    ) -> Result<LibraryCommandReceipt, LibraryBoundaryError> {
        if envelope.expected_revision != self.revision {
            return Err(LibraryBoundaryError::StaleRevision {
                expected: envelope.expected_revision,
                current: self.revision,
            });
        }
        let snapshot_catalog = self.catalog.clone();
        let snapshot_tags = self.tags.clone();
        let snapshot_names = self.tag_name_to_id.clone();
        let snapshot_item_revs = self.item_revisions.clone();
        let snapshot_next_id = self.next_id;
        let next_rev = self.revision.next();

        let outcome: Result<(Option<TagId>, bool), LibraryBoundaryError> =
            (|| match envelope.command {
                LibraryCommand::CreateTag { name, parent } => {
                    if self.tags.len() >= MAX_TAXONOMY_TAGS {
                        return Err(LibraryBoundaryError::LimitReached(
                            "taxonomy tag limit reached",
                        ));
                    }
                    let normalized = normalize_tag_input(&name)?;
                    if self.tag_name_to_id.contains_key(&normalized) {
                        return Err(LibraryBoundaryError::DuplicateTag(normalized));
                    }
                    self.tag_depth(parent)?;
                    let id = TagId(self.allocate_id());
                    self.tag_name_to_id.insert(normalized.clone(), id);
                    self.tags.insert(
                        id,
                        TagDefinition {
                            id,
                            name: normalized,
                            parent,
                        },
                    );
                    Ok((Some(id), true))
                }
                LibraryCommand::RenameTag { tag, name } => {
                    let Some(old_name) = self.tags.get(&tag).map(|t| t.name.clone()) else {
                        return Err(LibraryBoundaryError::UnknownTag(tag));
                    };
                    let normalized = normalize_tag_input(&name)?;
                    if old_name == normalized {
                        return Ok((None, false));
                    }
                    if self.tag_name_to_id.contains_key(&normalized) {
                        return Err(LibraryBoundaryError::DuplicateTag(normalized));
                    }
                    self.tag_name_to_id.remove(&old_name);
                    self.tag_name_to_id.insert(normalized.clone(), tag);
                    if let Some(def) = self.tags.get_mut(&tag) {
                        def.name = normalized.clone();
                    }
                    let affected: Vec<PathBuf> = self
                        .item_to_path
                        .values()
                        .filter(|p| {
                            self.catalog
                                .tags_for(p)
                                .is_some_and(|s| s.contains(&old_name))
                        })
                        .cloned()
                        .collect();
                    for path in affected {
                        self.catalog.remove_tag(&path, &old_name);
                        self.catalog
                            .add_tag(&path, &normalized)
                            .map_err(LibraryBoundaryError::LimitReached)?;
                        if let Some(&item_id) = self.path_to_item.get(&path) {
                            self.item_revisions.insert(item_id, next_rev);
                        }
                    }
                    Ok((None, true))
                }
                LibraryCommand::MergeTag { source, target } => {
                    if source == target {
                        return Err(LibraryBoundaryError::CycleDetected);
                    }
                    let Some(source_name) = self.tags.get(&source).map(|t| t.name.clone()) else {
                        return Err(LibraryBoundaryError::UnknownTag(source));
                    };
                    let Some(target_name) = self.tags.get(&target).map(|t| t.name.clone()) else {
                        return Err(LibraryBoundaryError::UnknownTag(target));
                    };
                    let descendants = self.descendant_tags(source);
                    if descendants.contains(&target) {
                        return Err(LibraryBoundaryError::CycleDetected);
                    }
                    let affected: Vec<PathBuf> = self
                        .item_to_path
                        .values()
                        .filter(|p| {
                            self.catalog
                                .tags_for(p)
                                .is_some_and(|s| s.contains(&source_name))
                        })
                        .cloned()
                        .collect();
                    for path in affected {
                        self.catalog.remove_tag(&path, &source_name);
                        self.catalog
                            .add_tag(&path, &target_name)
                            .map_err(LibraryBoundaryError::LimitReached)?;
                        if let Some(&item_id) = self.path_to_item.get(&path) {
                            self.item_revisions.insert(item_id, next_rev);
                        }
                    }
                    for def in self.tags.values_mut() {
                        if def.parent == Some(source) {
                            def.parent = Some(target);
                        }
                    }
                    self.tags.remove(&source);
                    self.tag_name_to_id.remove(&source_name);
                    Ok((None, true))
                }
                LibraryCommand::DeleteTag { tag } => {
                    let Some(def) = self.tags.get(&tag).cloned() else {
                        return Err(LibraryBoundaryError::UnknownTag(tag));
                    };
                    let affected: Vec<PathBuf> = self
                        .item_to_path
                        .values()
                        .filter(|p| {
                            self.catalog
                                .tags_for(p)
                                .is_some_and(|s| s.contains(&def.name))
                        })
                        .cloned()
                        .collect();
                    for path in affected {
                        self.catalog.remove_tag(&path, &def.name);
                        if let Some(&item_id) = self.path_to_item.get(&path) {
                            self.item_revisions.insert(item_id, next_rev);
                        }
                    }
                    for child in self.tags.values_mut() {
                        if child.parent == Some(tag) {
                            child.parent = def.parent;
                        }
                    }
                    self.tags.remove(&tag);
                    self.tag_name_to_id.remove(&def.name);
                    Ok((None, true))
                }
                LibraryCommand::AssignTag { item, tag } => {
                    let path = self
                        .item_to_path
                        .get(&item)
                        .cloned()
                        .ok_or(LibraryBoundaryError::UnknownItem(item))?;
                    let tag_name = self
                        .tags
                        .get(&tag)
                        .map(|t| t.name.clone())
                        .ok_or(LibraryBoundaryError::UnknownTag(tag))?;
                    let changed = self
                        .catalog
                        .add_tag(&path, &tag_name)
                        .map_err(LibraryBoundaryError::LimitReached)?;
                    if changed {
                        self.item_revisions.insert(item, next_rev);
                    }
                    Ok((None, changed))
                }
                LibraryCommand::RemoveTag { item, tag } => {
                    let path = self
                        .item_to_path
                        .get(&item)
                        .cloned()
                        .ok_or(LibraryBoundaryError::UnknownItem(item))?;
                    let tag_name = self
                        .tags
                        .get(&tag)
                        .map(|t| t.name.clone())
                        .ok_or(LibraryBoundaryError::UnknownTag(tag))?;
                    let changed = self.catalog.remove_tag(&path, &tag_name);
                    if changed {
                        self.item_revisions.insert(item, next_rev);
                    }
                    Ok((None, changed))
                }
                LibraryCommand::BulkAssignTag { items, tag } => {
                    let tag_name = self
                        .tags
                        .get(&tag)
                        .map(|t| t.name.clone())
                        .ok_or(LibraryBoundaryError::UnknownTag(tag))?;
                    let mut paths = Vec::with_capacity(items.len());
                    for item in &items {
                        let path = self
                            .item_to_path
                            .get(item)
                            .cloned()
                            .ok_or(LibraryBoundaryError::UnknownItem(*item))?;
                        paths.push((*item, path));
                    }
                    let mut any_changed = false;
                    for (item, path) in paths {
                        let changed = self
                            .catalog
                            .add_tag(&path, &tag_name)
                            .map_err(LibraryBoundaryError::LimitReached)?;
                        if changed {
                            self.item_revisions.insert(item, next_rev);
                            any_changed = true;
                        }
                    }
                    Ok((None, any_changed))
                }
                LibraryCommand::BulkRemoveTag { items, tag } => {
                    let tag_name = self
                        .tags
                        .get(&tag)
                        .map(|t| t.name.clone())
                        .ok_or(LibraryBoundaryError::UnknownTag(tag))?;
                    let mut paths = Vec::with_capacity(items.len());
                    for item in &items {
                        let path = self
                            .item_to_path
                            .get(item)
                            .cloned()
                            .ok_or(LibraryBoundaryError::UnknownItem(*item))?;
                        paths.push((*item, path));
                    }
                    let mut any_changed = false;
                    for (item, path) in paths {
                        if self.catalog.remove_tag(&path, &tag_name) {
                            self.item_revisions.insert(item, next_rev);
                            any_changed = true;
                        }
                    }
                    Ok((None, any_changed))
                }
            })();

        let (created_tag, changed) = match outcome {
            Ok(result) => result,
            Err(err) => {
                self.catalog = snapshot_catalog;
                self.tags = snapshot_tags;
                self.tag_name_to_id = snapshot_names;
                self.item_revisions = snapshot_item_revs;
                self.next_id = snapshot_next_id;
                return Err(err);
            }
        };

        if changed {
            if let Err(err) = self.persist_if_backed() {
                self.catalog = snapshot_catalog;
                self.tags = snapshot_tags;
                self.tag_name_to_id = snapshot_names;
                self.item_revisions = snapshot_item_revs;
                self.next_id = snapshot_next_id;
                return Err(err);
            }
            self.revision = next_rev;
        }
        Ok(LibraryCommandReceipt {
            committed_revision: self.revision,
            created_tag,
            changed,
        })
    }
}

fn normalize_tag_input(raw: &str) -> Result<String, LibraryBoundaryError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_TAG_BYTES {
        return Err(LibraryBoundaryError::InvalidLabel(
            "tag label must be non-empty and within byte limit",
        ));
    }
    normalize_label(trimmed).map_err(LibraryBoundaryError::InvalidLabel)
}
