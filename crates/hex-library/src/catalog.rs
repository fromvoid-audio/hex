//! Recursive, bounded sample discovery. All calls belong on control workers.
use crate::{
    DirectoryListing, EntryKind, LibraryEntry, MAX_RETAINED_ENTRIES, is_hidden, is_wav,
    sort_entries,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering},
    time::UNIX_EPOCH,
};

mod database;
mod fingerprint;
mod header;
mod lookup;
#[cfg(test)]
mod tests;

pub use fingerprint::{FINGERPRINT_BLOCK, Fingerprint};
pub use header::{AudioCodec, AudioHeaderMetadata, parse_audio_header};
pub use lookup::{CatalogMatch, MAX_CANDIDATES, MatchKind, MissingSample};

pub const MAX_SAMPLES: usize = 50_000;
pub const MAX_SCAN_ENTRIES: usize = 500_000;
pub const MAX_SCAN_DEPTH: usize = 64;
pub const MAX_DIRECTORIES: usize = 16_384;
pub const MAX_ROOTS: usize = 16;
/// Maximum distinct user tags attached to a single sample path.
pub const MAX_TAGS_PER_SAMPLE: usize = 32;
/// Maximum UTF-8 byte length of a normalized user tag or collection name.
pub const MAX_TAG_BYTES: usize = 64;
/// Maximum persistent user collections stored in one sample catalog.
pub const MAX_COLLECTIONS: usize = 64;
pub(super) const MAX_PATH_BYTES: usize = 16_384;
pub(super) const MAX_CATALOG_BYTES: usize = 32 * 1024 * 1024;

/// A derived sample database with audio header metadata and persistent user tags/collections.
/// Original paths remain the import/preview identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SampleCatalog {
    entries: Vec<LibraryEntry>,
    /// File facts keyed by original path. Entries loaded from a version 1 cache
    /// have no record until the next scan inspects them.
    records: HashMap<PathBuf, SampleRecord>,
    /// Persistent lowercase user tags keyed by original sample path.
    tags: HashMap<PathBuf, BTreeSet<String>>,
    /// Persistent named collections of original sample paths.
    collections: BTreeMap<String, BTreeSet<PathBuf>>,
}

/// Facts recorded for one sample file when it was last scanned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleRecord {
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch; 0 when unavailable.
    pub modified_ns: u64,
    /// `None` when the file could not be read; such entries stay browseable.
    pub fingerprint: Option<Fingerprint>,
    /// Lightweight audio header facts captured during the single fingerprint read.
    pub audio: Option<AudioHeaderMetadata>,
    /// Whether lightweight audio header parsing was attempted for this record
    /// (`false` for legacy `hexsmp2` records so the next scan populates metadata).
    pub audio_inspected: bool,
}

/// Sort order for metadata-aware browser queries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CatalogSort {
    #[default]
    NameAsc,
    NameDesc,
    DurationAsc,
    DurationDesc,
    SampleRateDesc,
    SampleRateAsc,
    BitDepthDesc,
    SizeDesc,
}

/// Structured query combining free-text search, user tags/collections, audio header
/// filters, and deterministic sort order. Runs purely in memory with zero disk I/O.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogQuery<'a> {
    pub text: &'a str,
    pub source: Option<&'a Path>,
    pub tag: Option<&'a str>,
    pub collection: Option<&'a str>,
    pub codec: Option<AudioCodec>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub min_duration_ms: Option<u64>,
    pub max_duration_ms: Option<u64>,
    pub sort: CatalogSort,
}

#[derive(Debug, Default)]
pub struct CatalogScan {
    pub catalog: SampleCatalog,
    /// Bounded per-folder errors; a failed folder does not hide other roots.
    pub errors: Vec<String>,
    pub truncated: bool,
    /// The scan stopped early on request; its catalog is partial and must not
    /// replace a complete one.
    pub cancelled: bool,
    /// Configured roots that no longer exist, in input order.
    pub missing_roots: Vec<PathBuf>,
    /// Files fingerprinted by reading their contents during this scan.
    pub fingerprinted: usize,
    /// Files whose previous record was reused because size and mtime matched.
    pub reused: usize,
    /// Sample files that could not be inspected or read.
    pub unreadable: usize,
}

/// Optional inputs for an incremental, cancellable scan.
#[derive(Clone, Copy, Default)]
pub struct ScanControl<'a> {
    /// The last catalog; unchanged files (same size and mtime) reuse its records.
    pub previous: Option<&'a SampleCatalog>,
    /// Set from any thread to stop the scan at the next entry.
    pub cancel: Option<&'a AtomicBool>,
    /// Incremented once per inspected directory entry, for progress display.
    pub progress: Option<&'a AtomicUsize>,
}

impl SampleCatalog {
    pub fn entries(&self) -> &[LibraryEntry] {
        &self.entries
    }

    pub fn record(&self, path: &Path) -> Option<&SampleRecord> {
        self.records.get(path)
    }

    pub fn audio_metadata(&self, path: &Path) -> Option<AudioHeaderMetadata> {
        self.records.get(path).and_then(|record| record.audio)
    }

    /// Attach a normalized lowercase tag to `path`. Returns `Ok(true)` if newly added.
    pub fn add_tag(&mut self, path: &Path, tag: &str) -> Result<bool, &'static str> {
        validate_sample_path(path)?;
        let normalized = normalize_label(tag)?;
        if let Some(set) = self.tags.get_mut(path) {
            if set.contains(&normalized) {
                return Ok(false);
            }
            if set.len() >= MAX_TAGS_PER_SAMPLE {
                return Err("sample tag limit reached");
            }
            return Ok(set.insert(normalized));
        }
        if self.tags.len() >= MAX_SAMPLES {
            return Err("tagged sample limit reached");
        }
        let mut set = BTreeSet::new();
        set.insert(normalized);
        self.tags.insert(path.to_path_buf(), set);
        Ok(true)
    }

    /// Remove a tag from `path`. Returns `true` if the tag was present.
    pub fn remove_tag(&mut self, path: &Path, tag: &str) -> bool {
        let Ok(normalized) = normalize_label(tag) else {
            return false;
        };
        let removed = self
            .tags
            .get_mut(path)
            .is_some_and(|set| set.remove(&normalized));
        if self.tags.get(path).is_some_and(BTreeSet::is_empty) {
            self.tags.remove(path);
        }
        removed
    }

    /// Sorted tags attached to `path`.
    pub fn tags_for(&self, path: &Path) -> Option<&BTreeSet<String>> {
        self.tags.get(path)
    }

    /// All distinct tags currently present in the catalog, sorted lexicographically.
    pub fn all_tags(&self) -> Vec<String> {
        let mut all = BTreeSet::new();
        for set in self.tags.values() {
            for tag in set {
                all.insert(tag.clone());
            }
        }
        all.into_iter().collect()
    }

    /// Create an empty named user collection. Returns `Ok(true)` if newly created.
    pub fn create_collection(&mut self, name: &str) -> Result<bool, &'static str> {
        let normalized = normalize_label(name)?;
        if self.collections.contains_key(&normalized) {
            return Ok(false);
        }
        if self.collections.len() >= MAX_COLLECTIONS {
            return Err("collection limit reached");
        }
        self.collections.insert(normalized, BTreeSet::new());
        Ok(true)
    }

    /// Delete a named collection. Returns `true` if it existed.
    pub fn delete_collection(&mut self, name: &str) -> bool {
        let Ok(normalized) = normalize_label(name) else {
            return false;
        };
        self.collections.remove(&normalized).is_some()
    }

    /// Add a sample path to a collection (creating the collection if bounded room exists).
    pub fn add_to_collection(
        &mut self,
        collection: &str,
        path: &Path,
    ) -> Result<bool, &'static str> {
        validate_sample_path(path)?;
        let normalized = normalize_label(collection)?;
        if !self.collections.contains_key(&normalized) {
            if self.collections.len() >= MAX_COLLECTIONS {
                return Err("collection limit reached");
            }
            self.collections.insert(normalized.clone(), BTreeSet::new());
        }
        let members = self
            .collections
            .get_mut(&normalized)
            .ok_or("collection is missing")?;
        if members.contains(path) {
            return Ok(false);
        }
        if members.len() >= MAX_SAMPLES {
            return Err("collection member limit reached");
        }
        Ok(members.insert(path.to_path_buf()))
    }

    /// Remove a sample path from a collection. Returns `true` if it was present.
    pub fn remove_from_collection(&mut self, collection: &str, path: &Path) -> bool {
        let Ok(normalized) = normalize_label(collection) else {
            return false;
        };
        self.collections
            .get_mut(&normalized)
            .is_some_and(|members| members.remove(path))
    }

    /// Sorted names of all persistent collections.
    pub fn collections(&self) -> Vec<String> {
        self.collections.keys().cloned().collect()
    }

    /// Member paths belonging to `collection`.
    pub fn collection_members(&self, collection: &str) -> Option<&BTreeSet<PathBuf>> {
        let normalized = normalize_label(collection).ok()?;
        self.collections.get(&normalized)
    }

    /// Sorted collection names that contain `path`.
    pub fn collections_for(&self, path: &Path) -> Vec<String> {
        self.collections
            .iter()
            .filter(|(_, members)| members.contains(path))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Re-key records, tags, and collections under `old` to the same relative paths
    /// under `new` after a place was relocated, so a later incremental scan can reuse
    /// fingerprints of moved files whose size and mtime are unchanged. Paths outside `old` stay.
    pub fn relocate_root(&mut self, old: &Path, new: &Path) {
        if old == new || !old.is_absolute() || !new.is_absolute() {
            return;
        }
        let mut moved = false;
        for entry in &mut self.entries {
            let Some(path) = remap_under_root(&entry.path, old, new) else {
                continue;
            };
            if let Some(record) = self.records.remove(&entry.path) {
                self.records.insert(path.clone(), record);
            }
            entry.path = path;
            moved = true;
        }
        // Remap persistent user tags and collection memberships under `old`, keeping
        // merged tag sets bounded by MAX_TAGS_PER_SAMPLE and deterministic.
        let mut remapped_tags: Vec<(PathBuf, BTreeSet<String>)> = self
            .tags
            .drain()
            .map(|(path, set)| {
                let next = remap_under_root(&path, old, new).unwrap_or(path);
                (next, set)
            })
            .collect();
        remapped_tags.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, set) in remapped_tags {
            if set.is_empty() {
                continue;
            }
            let target = self.tags.entry(path).or_default();
            for tag in set {
                if target.len() >= MAX_TAGS_PER_SAMPLE {
                    break;
                }
                target.insert(tag);
            }
        }
        for members in self.collections.values_mut() {
            let remapped: BTreeSet<PathBuf> = members
                .iter()
                .map(|path| remap_under_root(path, old, new).unwrap_or_else(|| path.clone()))
                .collect();
            *members = remapped;
        }
        // A relocated entry may now alias one already cataloged under `new`.
        let mut seen = BTreeSet::new();
        self.entries.retain(|entry| seen.insert(entry.path.clone()));
        if moved {
            sort_entries(&mut self.entries, true);
        }
    }

    /// Combine global sample matches with a bounded current-folder snapshot.
    /// Original-path identity deduplicates WAVs present in both sources while
    /// retaining current-folder WAVs outside configured sample roots. This does
    /// not enumerate files or recursively search projects.
    pub fn search_with_listing(
        &self,
        query: &str,
        listing: Option<&DirectoryListing>,
    ) -> Vec<LibraryEntry> {
        self.search_with_listing_in(query, None, listing)
    }

    /// The same indexed/current-folder composition scoped to one active Place.
    pub fn search_with_listing_in(
        &self,
        query: &str,
        source: Option<&Path>,
        listing: Option<&DirectoryListing>,
    ) -> Vec<LibraryEntry> {
        let lowered = query.to_lowercase();
        let parsed = parse_query_tokens(&lowered);
        if parsed.impossible {
            return Vec::new();
        }
        let mut entries = self.query_parsed(
            &CatalogQuery {
                text: query,
                source,
                ..CatalogQuery::default()
            },
            &parsed,
        );
        let mut folded = String::new();
        if let Some(listing) = listing {
            let collection_set = match parsed.inline_collection {
                Some(name) => match self.collections.get(name) {
                    Some(members) => Some(members),
                    None => return entries,
                },
                None => None,
            };
            let extra = {
                let mut seen = entries
                    .iter()
                    .map(|entry| entry.path.as_path())
                    .collect::<BTreeSet<_>>();
                listing
                    .entries
                    .iter()
                    .take(MAX_RETAINED_ENTRIES)
                    .filter(|entry| source.is_none_or(|root| entry.path.starts_with(root)))
                    .filter(|entry| {
                        if let Some(members) = collection_set
                            && !members.contains(&entry.path)
                        {
                            return false;
                        }
                        let entry_tags = self.tags.get(&entry.path);
                        for tag in &parsed.inline_tags {
                            if !entry_tags.is_some_and(|set| set.contains(*tag)) {
                                return false;
                            }
                        }
                        let audio = self.records.get(&entry.path).and_then(|r| r.audio);
                        if let Some(codec) = parsed.inline_codec
                            && audio.is_none_or(|m| m.codec != codec)
                        {
                            return false;
                        }
                        if let Some(rate) = parsed.inline_rate
                            && audio.is_none_or(|m| m.sample_rate != rate)
                        {
                            return false;
                        }
                        if let Some(ch) = parsed.inline_channels
                            && audio.is_none_or(|m| m.channels != ch)
                        {
                            return false;
                        }
                        if source.is_some() {
                            matches_path_or_tags(
                                &entry.path,
                                entry_tags,
                                &parsed.text_terms,
                                &mut folded,
                            )
                        } else {
                            matches_name_terms(&entry.name, &parsed.text_terms, &mut folded)
                        }
                    })
                    .filter(|entry| seen.insert(entry.path.as_path()))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            entries.extend(extra);
        }
        sort_entries(&mut entries, true);
        entries
    }

    /// Discard cached records outside the current configured roots.
    pub fn retain_roots(&mut self, roots: &[PathBuf]) {
        self.entries
            .retain(|entry| roots.iter().any(|root| entry.path.starts_with(root)));
        let entries = &self.entries;
        if self.records.len() > entries.len() {
            let kept: BTreeSet<_> = entries.iter().map(|entry| entry.path.as_path()).collect();
            self.records.retain(|path, _| kept.contains(path.as_path()));
        }
    }

    /// Match all whitespace-separated terms against original path/name, user tags,
    /// and inline metadata tokens (`tag:`, `col:`, `rate:`, `ch:`, `codec:`).
    pub fn filtered(&self, query: &str, source: Option<&Path>) -> Vec<LibraryEntry> {
        self.query(&CatalogQuery {
            text: query,
            source,
            ..CatalogQuery::default()
        })
    }

    /// Execute a metadata-aware, tag/collection-aware query with deterministic sorting.
    /// Operates purely in memory over indexed catalog state with zero disk I/O.
    pub fn query(&self, q: &CatalogQuery<'_>) -> Vec<LibraryEntry> {
        let lowered = q.text.to_lowercase();
        let parsed = parse_query_tokens(&lowered);
        if parsed.impossible {
            return Vec::new();
        }
        self.query_parsed(q, &parsed)
    }

    fn query_parsed(&self, q: &CatalogQuery<'_>, parsed: &ParsedQuery<'_>) -> Vec<LibraryEntry> {
        let required_tag = match q.tag {
            Some(tag) => match normalize_label(tag) {
                Ok(normalized) => Some(normalized),
                Err(_) => return Vec::new(),
            },
            None => None,
        };
        let explicit_collection = match q.collection {
            Some(collection) => match normalize_label(collection) {
                Ok(normalized) => Some(normalized),
                Err(_) => return Vec::new(),
            },
            None => None,
        };
        if let (Some(explicit), Some(inline)) =
            (explicit_collection.as_deref(), parsed.inline_collection)
            && explicit != inline
        {
            return Vec::new();
        }
        let required_collection =
            explicit_collection.or_else(|| parsed.inline_collection.map(str::to_owned));
        let collection_set = match required_collection.as_deref() {
            Some(name) => match self.collections.get(name) {
                Some(members) => Some(members),
                None => return Vec::new(),
            },
            None => None,
        };
        let required_rate = q.sample_rate.or(parsed.inline_rate);
        let required_channels = q.channels.or(parsed.inline_channels);
        let required_codec = q.codec.or(parsed.inline_codec);

        let mut folded = String::new();
        let mut results: Vec<LibraryEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                if !q.source.is_none_or(|source| entry.path.starts_with(source)) {
                    return false;
                }
                if let Some(members) = collection_set
                    && !members.contains(&entry.path)
                {
                    return false;
                }
                let entry_tags = self.tags.get(&entry.path);
                if let Some(tag) = required_tag.as_deref()
                    && !entry_tags.is_some_and(|set| set.contains(tag))
                {
                    return false;
                }
                for tag in &parsed.inline_tags {
                    if !entry_tags.is_some_and(|set| set.contains(*tag)) {
                        return false;
                    }
                }
                let audio = self.records.get(&entry.path).and_then(|r| r.audio);
                if let Some(codec) = required_codec
                    && audio.is_none_or(|m| m.codec != codec)
                {
                    return false;
                }
                if let Some(rate) = required_rate
                    && audio.is_none_or(|m| m.sample_rate != rate)
                {
                    return false;
                }
                if let Some(ch) = required_channels
                    && audio.is_none_or(|m| m.channels != ch)
                {
                    return false;
                }
                if let Some(min_ms) = q.min_duration_ms
                    && audio.is_none_or(|m| m.duration_ms() < min_ms)
                {
                    return false;
                }
                if let Some(max_ms) = q.max_duration_ms
                    && audio.is_none_or(|m| m.duration_ms() > max_ms)
                {
                    return false;
                }
                matches_path_or_tags(&entry.path, entry_tags, &parsed.text_terms, &mut folded)
            })
            .cloned()
            .collect();

        match q.sort {
            CatalogSort::NameAsc => {} // `self.entries` is already sorted NameAsc with path tie-break.
            CatalogSort::NameDesc => results.reverse(),
            CatalogSort::DurationAsc => {
                results.sort_by(|a, b| {
                    let da = self
                        .audio_metadata(&a.path)
                        .map_or(u64::MAX, |m| m.duration_ms());
                    let db = self
                        .audio_metadata(&b.path)
                        .map_or(u64::MAX, |m| m.duration_ms());
                    da.cmp(&db).then_with(|| a.path.cmp(&b.path))
                });
            }
            CatalogSort::DurationDesc => {
                results.sort_by(|a, b| {
                    let da = self.audio_metadata(&a.path).map_or(0, |m| m.duration_ms());
                    let db = self.audio_metadata(&b.path).map_or(0, |m| m.duration_ms());
                    db.cmp(&da).then_with(|| a.path.cmp(&b.path))
                });
            }
            CatalogSort::SampleRateDesc => {
                results.sort_by(|a, b| {
                    let ra = self.audio_metadata(&a.path).map_or(0, |m| m.sample_rate);
                    let rb = self.audio_metadata(&b.path).map_or(0, |m| m.sample_rate);
                    rb.cmp(&ra).then_with(|| a.path.cmp(&b.path))
                });
            }
            CatalogSort::SampleRateAsc => {
                results.sort_by(|a, b| {
                    let ra = self
                        .audio_metadata(&a.path)
                        .map_or(u32::MAX, |m| m.sample_rate);
                    let rb = self
                        .audio_metadata(&b.path)
                        .map_or(u32::MAX, |m| m.sample_rate);
                    ra.cmp(&rb).then_with(|| a.path.cmp(&b.path))
                });
            }
            CatalogSort::BitDepthDesc => {
                results.sort_by(|a, b| {
                    let ba = self.audio_metadata(&a.path).map_or(0, |m| m.bit_depth);
                    let bb = self.audio_metadata(&b.path).map_or(0, |m| m.bit_depth);
                    bb.cmp(&ba).then_with(|| a.path.cmp(&b.path))
                });
            }
            CatalogSort::SizeDesc => {
                results.sort_by(|a, b| {
                    let sa = self.records.get(&a.path).map_or(0, |r| r.size);
                    let sb = self.records.get(&b.path).map_or(0, |r| r.size);
                    sb.cmp(&sa).then_with(|| a.path.cmp(&b.path))
                });
            }
        }
        results
    }
}

struct ParsedQuery<'a> {
    text_terms: Vec<&'a str>,
    inline_tags: Vec<&'a str>,
    inline_collection: Option<&'a str>,
    inline_rate: Option<u32>,
    inline_channels: Option<u16>,
    inline_codec: Option<AudioCodec>,
    impossible: bool,
}

fn parse_query_tokens(lowered: &str) -> ParsedQuery<'_> {
    let mut parsed = ParsedQuery {
        text_terms: Vec::new(),
        inline_tags: Vec::new(),
        inline_collection: None,
        inline_rate: None,
        inline_channels: None,
        inline_codec: None,
        impossible: false,
    };
    for token in lowered.split_whitespace() {
        if let Some(val) = token.strip_prefix("tag:") {
            if !val.is_empty() {
                if normalize_label(val).is_ok() {
                    parsed.inline_tags.push(val);
                } else {
                    parsed.impossible = true;
                }
            }
        } else if let Some(val) = token.strip_prefix("col:") {
            if !val.is_empty() {
                if normalize_label(val).is_err()
                    || parsed
                        .inline_collection
                        .is_some_and(|existing| existing != val)
                {
                    parsed.impossible = true;
                } else {
                    parsed.inline_collection = Some(val);
                }
            }
        } else if let Some(val) = token.strip_prefix("rate:") {
            if let Ok(sr) = val.parse::<u32>() {
                if parsed.inline_rate.is_some_and(|existing| existing != sr) {
                    parsed.impossible = true;
                } else {
                    parsed.inline_rate = Some(sr);
                }
            } else {
                parsed.impossible = true;
            }
        } else if let Some(val) = token.strip_prefix("ch:") {
            if let Ok(ch) = val.parse::<u16>() {
                if parsed
                    .inline_channels
                    .is_some_and(|existing| existing != ch)
                {
                    parsed.impossible = true;
                } else {
                    parsed.inline_channels = Some(ch);
                }
            } else {
                parsed.impossible = true;
            }
        } else if let Some(val) = token.strip_prefix("codec:") {
            if let Some(codec) = AudioCodec::from_token(val) {
                if parsed
                    .inline_codec
                    .is_some_and(|existing| existing != codec)
                {
                    parsed.impossible = true;
                } else {
                    parsed.inline_codec = Some(codec);
                }
            } else {
                parsed.impossible = true;
            }
        } else {
            parsed.text_terms.push(token);
        }
    }
    parsed
}

fn encoded_path_len(path: &Path) -> usize {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().len()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().count().saturating_mul(2)
    }
    #[cfg(not(any(unix, windows)))]
    {
        path.to_string_lossy().len()
    }
}

fn validate_sample_path(path: &Path) -> Result<(), &'static str> {
    if !path.is_absolute() {
        return Err("sample path must be absolute");
    }
    let len = encoded_path_len(path);
    if len == 0 || len > MAX_PATH_BYTES {
        return Err("sample path exceeds size limit");
    }
    Ok(())
}

fn remap_under_root(path: &Path, old: &Path, new: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(old).ok()?;
    let candidate = new.join(relative);
    validate_sample_path(&candidate).ok()?;
    Some(candidate)
}

pub(super) fn normalize_label(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("label must not be empty");
    }
    let normalized = trimmed.to_lowercase();
    if normalized.len() > MAX_TAG_BYTES {
        return Err("label exceeds 64 bytes");
    }
    if normalized.chars().any(char::is_control) {
        return Err("label must not contain control characters");
    }
    Ok(normalized)
}

fn matches_path_or_tags(
    path: &Path,
    tags: Option<&BTreeSet<String>>,
    terms: &[&str],
    folded: &mut String,
) -> bool {
    if terms.is_empty() {
        return true;
    }
    let path_str = path.to_string_lossy();
    let lowered = lowercase_into(&path_str, folded);
    terms.iter().all(|term| {
        lowered.contains(term) || tags.is_some_and(|set| set.iter().any(|tag| tag.contains(term)))
    })
}

fn matches_name_terms(name: &str, terms: &[&str], folded: &mut String) -> bool {
    if terms.is_empty() {
        return true;
    }
    let lowered = lowercase_into(name, folded);
    terms.iter().all(|term| lowered.contains(term))
}

/// Reuse query-local ASCII capacity. Full Unicode lowering stays with `str` so
/// contextual mappings (such as final Greek sigma) retain existing search semantics.
fn lowercase_into<'a>(value: &str, folded: &'a mut String) -> &'a str {
    if value.is_ascii() {
        folded.clear();
        folded.push_str(value);
        folded.make_ascii_lowercase();
    } else {
        *folded = value.to_lowercase();
    }
    folded.as_str()
}

/// Scan Music and explicitly added places, skipping hidden entries and directory
/// symlinks. Canonical directory identities prevent overlapping roots from repeating
/// work. The supplied paths, including non-UTF8 names, are preserved in results.
pub fn scan_samples(roots: &[PathBuf]) -> CatalogScan {
    scan_samples_with(roots, ScanControl::default())
}

/// [`scan_samples`] with incremental reuse, cancellation and progress. Sample
/// files are opened read-only; nothing under a root is created, moved or changed.
pub fn scan_samples_with(roots: &[PathBuf], control: ScanControl<'_>) -> CatalogScan {
    let mut walk = Walk {
        control,
        visited: BTreeSet::new(),
        inspected: 0,
        path_bytes: 0,
        result: CatalogScan {
            truncated: roots.len() > MAX_ROOTS,
            ..CatalogScan::default()
        },
    };
    for root in roots.iter().take(MAX_ROOTS) {
        if walk.cancelled() {
            break;
        }
        if !root.is_absolute() {
            walk.report(
                root,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "sample folders must be absolute paths",
                ),
            );
            continue;
        }
        if matches!(fs::symlink_metadata(root), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            walk.result.missing_roots.push(root.clone());
        }
        walk.visit(root, 0);
    }
    let mut result = walk.result;
    if let Some(previous) = control.previous {
        result.catalog.tags = previous.tags.clone();
        result.catalog.collections = previous.collections.clone();
    }
    sort_entries(&mut result.catalog.entries, true);
    result
}

struct Walk<'a> {
    control: ScanControl<'a>,
    visited: BTreeSet<PathBuf>,
    inspected: usize,
    path_bytes: usize,
    result: CatalogScan,
}

impl Walk<'_> {
    fn cancelled(&mut self) -> bool {
        if !self.result.cancelled
            && self
                .control
                .cancel
                .is_some_and(|cancel| cancel.load(AtomicOrdering::Relaxed))
        {
            self.result.cancelled = true;
        }
        self.result.cancelled
    }

    fn full(&mut self) -> bool {
        if self.inspected >= MAX_SCAN_ENTRIES || self.result.catalog.entries.len() >= MAX_SAMPLES {
            self.result.truncated = true;
            return true;
        }
        false
    }

    fn report(&mut self, path: &Path, error: std::io::Error) {
        if self.result.errors.len() < MAX_ROOTS {
            self.result
                .errors
                .push(format!("could not scan {}: {error}", path.display()));
        }
    }

    fn visit(&mut self, directory: &Path, depth: usize) {
        if self.full() {
            return;
        }
        let identity = match fs::canonicalize(directory) {
            Ok(path) => path,
            Err(error) => {
                self.report(directory, error);
                return;
            }
        };
        let identity_bytes = identity.as_os_str().as_encoded_bytes().len();
        if self.visited.len() >= MAX_DIRECTORIES
            || identity_bytes > MAX_PATH_BYTES
            || self.path_bytes + identity_bytes > MAX_CATALOG_BYTES / 2
        {
            self.result.truncated = true;
            return;
        }
        if !self.visited.insert(identity) {
            return;
        }
        self.path_bytes += identity_bytes;
        let items = match fs::read_dir(directory) {
            Ok(items) => items,
            Err(error) => {
                self.report(directory, error);
                return;
            }
        };
        for item in items {
            if self.cancelled() || self.full() {
                break;
            }
            self.inspected += 1;
            if let Some(progress) = self.control.progress {
                progress.fetch_add(1, AtomicOrdering::Relaxed);
            }
            let item = match item {
                Ok(item) => item,
                Err(error) => {
                    self.report(directory, error);
                    continue;
                }
            };
            if is_hidden(&item.file_name()) {
                continue;
            }
            let path = item.path();
            let kind = match item.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    self.report(&path, error);
                    continue;
                }
            };
            if kind.is_dir() {
                if depth < MAX_SCAN_DEPTH {
                    self.visit(&path, depth + 1);
                } else {
                    self.result.truncated = true;
                }
            } else if kind.is_file() && is_wav(path.extension()) {
                let size = path.as_os_str().as_encoded_bytes().len();
                if size > MAX_PATH_BYTES || self.path_bytes + size > MAX_CATALOG_BYTES / 2 {
                    self.result.truncated = true;
                    continue;
                }
                self.path_bytes += size;
                if let Some(record) = self.record(&item, &path) {
                    self.result.catalog.records.insert(path.clone(), record);
                }
                self.result.catalog.entries.push(LibraryEntry {
                    name: item.file_name().to_string_lossy().into_owned(),
                    path,
                    kind: EntryKind::WaveAudio,
                });
            }
        }
    }

    /// Reuse the previous record when size and mtime are unchanged; otherwise
    /// fingerprint the file. Unreadable files stay listed without a fingerprint.
    fn record(&mut self, item: &fs::DirEntry, path: &Path) -> Option<SampleRecord> {
        let Ok(metadata) = item.metadata() else {
            self.result.unreadable += 1;
            return None;
        };
        let size = metadata.len();
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| {
                u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
            });
        if let Some(previous) = self
            .control
            .previous
            .and_then(|catalog| catalog.records.get(path))
            && previous.size == size
            && previous.modified_ns == modified_ns
            && modified_ns != 0
            && previous.fingerprint.is_some()
            && previous.audio_inspected
        {
            self.result.reused += 1;
            return Some(*previous);
        }
        let (fingerprint, audio, audio_inspected) = match Fingerprint::inspect_file(path) {
            Ok((fingerprint, audio)) => {
                self.result.fingerprinted += 1;
                (Some(fingerprint), audio, true)
            }
            Err(_) => {
                self.result.unreadable += 1;
                (None, None, false)
            }
        };
        Some(SampleRecord {
            size,
            modified_ns,
            fingerprint,
            audio,
            audio_inspected,
        })
    }
}
#[cfg(test)]
mod profile_test;
