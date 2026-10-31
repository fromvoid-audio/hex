//! Ranked candidate lookup for missing project media.
//!
//! This is the narrow contract relinking consumes; the catalog stays a derived,
//! replaceable index and never decides that a candidate is the right file. Callers
//! confirm candidates (for example by decoded frame count and rate) before use.
use super::{Fingerprint, SampleCatalog};
use std::{
    cmp::Ordering,
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Maximum candidates returned for one missing file.
pub const MAX_CANDIDATES: usize = 32;

/// What is known about a missing file. Only the file name is required.
#[derive(Clone, Copy, Debug)]
pub struct MissingSample<'a> {
    pub file_name: &'a OsStr,
    pub size: Option<u64>,
    pub fingerprint: Option<Fingerprint>,
}

impl<'a> MissingSample<'a> {
    /// A lookup by the missing path's file name alone.
    pub fn named(path: &'a Path) -> Option<Self> {
        Some(Self {
            file_name: path.file_name()?,
            size: None,
            fingerprint: None,
        })
    }
}

/// Why a candidate matched, strongest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// Same content fingerprint, whatever the name.
    Fingerprint,
    /// Same file name (ignoring case) and byte size.
    NameAndSize,
    /// Same file name (ignoring case).
    Name,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogMatch {
    pub path: PathBuf,
    pub kind: MatchKind,
}

impl SampleCatalog {
    /// Rank catalog entries for a missing file: fingerprint, then name + size,
    /// then name. Ties keep native path order. Bounded to [`MAX_CANDIDATES`]; an
    /// empty file name matches nothing.
    pub fn find_candidates(&self, missing: &MissingSample<'_>) -> Vec<CatalogMatch> {
        let wanted = missing.file_name.to_string_lossy().to_lowercase();
        let mut matches: Vec<_> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let record = self.records.get(&entry.path);
                let size = record.map(|record| record.size);
                let fingerprint_match = missing
                    .fingerprint
                    .is_some_and(|wanted| record.and_then(|r| r.fingerprint) == Some(wanted))
                    && missing.size.is_none_or(|wanted| size == Some(wanted));
                let kind = if fingerprint_match {
                    MatchKind::Fingerprint
                } else if wanted.is_empty() || entry.name.to_lowercase() != wanted {
                    return None;
                } else if missing.size.is_some() && missing.size == size {
                    MatchKind::NameAndSize
                } else {
                    MatchKind::Name
                };
                Some(CatalogMatch {
                    path: entry.path.clone(),
                    kind,
                })
            })
            .collect();
        matches.sort_by(|a, b| match a.kind.cmp(&b.kind) {
            Ordering::Equal => a.path.cmp(&b.path),
            order => order,
        });
        matches.truncate(MAX_CANDIDATES);
        matches
    }
}
