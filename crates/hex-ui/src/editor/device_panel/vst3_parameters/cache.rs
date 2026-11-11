//! Bounded display metadata cache for hosted VST3 parameter names and order.
//!
//! A cache hit is only a placeholder for the current controller query. Saved
//! values, editability, automation eligibility and revisions never cross this
//! boundary.
use hex_session::service::PluginParameterRow;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const VERSION: u64 = 2;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 2_624;
const NAME_UTF16_UNITS: usize = 127;
const PAGE_SIZE: usize = 4;
const MAX_KEY_BYTES: usize = 256;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, PartialEq)]
pub struct CachedInventory {
    pub descriptor_signature: [u8; 32],
    pub rows: Vec<PluginParameterRow>,
}

/// Load a display-only metadata cache. Missing, old-version, and different-key
/// entries are misses; malformed or linked files return an error for fallback.
pub fn load(path: &Path, key: &str) -> Result<Option<CachedInventory>, String> {
    validate_key(key)?;
    plain_path(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("vst3 parameter cache unavailable: {error}")),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_BYTES as u64 {
        return Err("vst3 parameter cache is not a bounded regular file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes))
        .map_err(|error| format!("vst3 parameter cache read failed: {error}"))?;
    if bytes.len() > MAX_BYTES {
        return Err("vst3 parameter cache exceeds its byte limit".into());
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "vst3 parameter cache is malformed".to_owned())?;
    let object = value
        .as_object()
        .ok_or_else(|| "vst3 parameter cache is malformed".to_owned())?;
    let version = object
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| "vst3 parameter cache version is malformed".to_owned())?;
    if version != VERSION {
        return Ok(None);
    }
    let stored_key = object
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| "vst3 parameter cache key is malformed".to_owned())?;
    if stored_key != key {
        return Ok(None);
    }
    if object.len() != 4 {
        return Err("vst3 parameter cache fields are malformed".into());
    }
    let digest = object
        .get("descriptor_signature")
        .and_then(Value::as_array)
        .filter(|digest| digest.len() == 32)
        .and_then(|digest| {
            let mut bytes = [0_u8; 32];
            for (index, byte) in digest.iter().enumerate() {
                bytes[index] = u8::try_from(byte.as_u64()?).ok()?;
            }
            Some(bytes)
        })
        .ok_or_else(|| "vst3 parameter cache signature is malformed".to_owned())?;
    let entries = object
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| "vst3 parameter cache rows are malformed".to_owned())?;
    decode_rows(entries).map(|rows| {
        Some(CachedInventory {
            descriptor_signature: digest,
            rows,
        })
    })
}

/// Atomically replace the bounded cache with stable display metadata only.
pub fn save(
    path: &Path,
    key: &str,
    descriptor_signature: &[u8; 32],
    rows: &[PluginParameterRow],
) -> Result<(), String> {
    validate_key(key)?;
    let entries = encode_rows(rows)?;
    let bytes = serde_json::to_vec(&json!({
        "version": VERSION,
        "key": key,
        "descriptor_signature": descriptor_signature,
        "rows": entries,
    }))
    .map_err(|_| "vst3 parameter cache encoding failed".to_owned())?;
    if bytes.len() > MAX_BYTES {
        return Err("vst3 parameter cache exceeds its byte limit".into());
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    plain_path(path)?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("vst3 parameter cache folder failed: {error}"))?;
    plain_path(path)?;

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "vst3 parameter cache filename is invalid".to_owned())?;
    let mut temporary = None;
    for _ in 0..8 {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{name}.{}-{serial}.pending", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((PendingFile(candidate), file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!(
                    "vst3 parameter cache temporary file failed: {error}"
                ));
            }
        }
    }
    let (pending, mut file) = temporary
        .ok_or_else(|| "vst3 parameter cache temporary file capacity exceeded".to_owned())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("vst3 parameter cache write failed: {error}"))?;
    drop(file);
    plain_path(path)?;
    fs::rename(&pending.0, path)
        .map_err(|error| format!("vst3 parameter cache publication failed: {error}"))?;
    let _ = File::open(parent).and_then(|directory| directory.sync_all());
    Ok(())
}

struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err("vst3 parameter cache key is invalid".into());
    }
    Ok(())
}

fn plain_path(path: &Path) -> Result<(), String> {
    for member in path.ancestors() {
        match fs::symlink_metadata(member) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("linked vst3 parameter cache path refused".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("vst3 parameter cache path unavailable: {error}")),
        }
    }
    Ok(())
}

fn encode_rows(rows: &[PluginParameterRow]) -> Result<Vec<Value>, String> {
    if rows.len() > MAX_ROWS {
        return Err("vst3 parameter cache row limit exceeded".into());
    }
    let mut ids = HashSet::with_capacity(rows.len());
    let mut encoded = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        validate_row(index, row, &mut ids)?;
        encoded.push(json!({
            "id": row.id,
            "name": row.name,
            "default_normalized": row.default_normalized,
            "steps": row.steps,
            "page_first": row.page_first,
        }));
    }
    Ok(encoded)
}

fn decode_rows(entries: &[Value]) -> Result<Vec<PluginParameterRow>, String> {
    if entries.len() > MAX_ROWS {
        return Err("vst3 parameter cache row limit exceeded".into());
    }
    let mut ids = HashSet::with_capacity(entries.len());
    let mut rows = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let object = entry
            .as_object()
            .ok_or_else(|| "vst3 parameter cache row is malformed".to_owned())?;
        if object.len() != 5 {
            return Err("vst3 parameter cache row fields are malformed".into());
        }
        let id = object
            .get("id")
            .and_then(Value::as_u64)
            .and_then(|id| u32::try_from(id).ok())
            .ok_or_else(|| "vst3 parameter cache ID is malformed".to_owned())?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "vst3 parameter cache name is malformed".to_owned())?
            .to_owned();
        let default_normalized = object
            .get("default_normalized")
            .and_then(Value::as_f64)
            .ok_or_else(|| "vst3 parameter cache default is malformed".to_owned())?;
        let steps = object
            .get("steps")
            .and_then(Value::as_u64)
            .and_then(|steps| u32::try_from(steps).ok())
            .ok_or_else(|| "vst3 parameter cache steps are malformed".to_owned())?;
        let page_first = object
            .get("page_first")
            .and_then(Value::as_u64)
            .and_then(|first| u16::try_from(first).ok())
            .ok_or_else(|| "vst3 parameter cache page is malformed".to_owned())?;
        let row = PluginParameterRow {
            id,
            name,
            value: String::new(),
            normalized: default_normalized,
            default_normalized,
            steps,
            editable: false,
            automation_class: None,
            page_first,
            metadata_revision: 0,
            editor_sequence: 0,
        };
        validate_row(index, &row, &mut ids)?;
        rows.push(row);
    }
    Ok(rows)
}

fn validate_row(
    index: usize,
    row: &PluginParameterRow,
    ids: &mut HashSet<u32>,
) -> Result<(), String> {
    let name_units = row.name.encode_utf16().count();
    if row.id > i32::MAX as u32
        || name_units == 0
        || name_units > NAME_UTF16_UNITS
        || row.name.contains('\0')
        || row.steps > i32::MAX as u32
        || !row.default_normalized.is_finite()
        || !(0.0..=1.0).contains(&row.default_normalized)
        || usize::from(row.page_first) != (index / PAGE_SIZE) * PAGE_SIZE
        || !ids.insert(row.id)
    {
        return Err("vst3 parameter cache row is invalid".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!(
                    "hex-vst3-parameter-cache-{}-{serial}",
                    std::process::id()
                ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self) -> PathBuf {
            self.0.join("parameters.json")
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn row(id: u32, name: &str, page_first: u16) -> PluginParameterRow {
        PluginParameterRow {
            id,
            name: name.into(),
            value: "live value".into(),
            normalized: 0.75,
            default_normalized: 0.25,
            steps: 7,
            editable: true,
            automation_class: Some([9; 16]),
            page_first,
            metadata_revision: 42,
            editor_sequence: 0,
        }
    }

    #[test]
    fn round_trip_restores_display_metadata_without_live_authority() {
        let directory = TestDir::new();
        let rows = vec![row(7, "gain", 0)];
        save(&directory.file(), "binary-class-v1", &[4; 32], &rows).unwrap();
        let cached = load(&directory.file(), "binary-class-v1").unwrap().unwrap();
        assert_eq!(cached.descriptor_signature, [4; 32]);
        assert_eq!(cached.rows[0].name, "gain");
        assert_eq!(cached.rows[0].default_normalized, 0.25);
        assert_eq!(cached.rows[0].normalized, 0.25);
        assert!(cached.rows[0].value.is_empty());
        assert!(!cached.rows[0].editable);
        assert_eq!(cached.rows[0].automation_class, None);
        assert_eq!(cached.rows[0].metadata_revision, 0);
    }

    #[test]
    fn missing_different_key_and_old_version_are_cache_misses() {
        let directory = TestDir::new();
        assert_eq!(load(&directory.file(), "key"), Ok(None));
        save(&directory.file(), "one", &[4; 32], &[row(1, "gain", 0)]).unwrap();
        assert_eq!(load(&directory.file(), "two"), Ok(None));
        fs::write(directory.file(), br#"{"version":1,"key":"one","rows":[]}"#).unwrap();
        assert_eq!(load(&directory.file(), "one"), Ok(None));
    }

    #[test]
    fn malformed_descriptor_signature_fails_closed() {
        let directory = TestDir::new();
        for signature in [
            json!(vec![7; 31]),
            json!(
                (0..32)
                    .map(|index| if index == 0 { 256 } else { 7 })
                    .collect::<Vec<_>>()
            ),
        ] {
            let bytes = serde_json::to_vec(&json!({
                "version": VERSION,
                "key": "key",
                "descriptor_signature": signature,
                "rows": [],
            }))
            .unwrap();
            fs::write(directory.file(), bytes).unwrap();
            assert!(load(&directory.file(), "key").is_err());
        }
    }

    #[test]
    fn corrupt_oversized_duplicate_and_invalid_rows_fail_closed() {
        let directory = TestDir::new();
        let path = directory.file();
        fs::write(&path, b"not json").unwrap();
        assert!(load(&path, "key").is_err());
        fs::write(&path, vec![b' '; MAX_BYTES + 1]).unwrap();
        assert!(load(&path, "key").is_err());
        assert!(
            save(
                &path,
                "key",
                &[4; 32],
                &[row(1, "gain", 0), row(1, "gain again", 0)]
            )
            .is_err()
        );
        assert!(save(&path, "key", &[4; 32], &[row(1, "", 0)]).is_err());
        assert!(save(&path, "key", &[4; 32], &[row(1, &"n".repeat(128), 0)]).is_err());
        assert!(save(&path, "key", &[4; 32], &[row(1, "gain\0bad", 0)]).is_err());
        assert!(
            save(
                &path,
                "key",
                &[4; 32],
                &[PluginParameterRow {
                    steps: i32::MAX as u32 + 1,
                    ..row(1, "gain", 0)
                }]
            )
            .is_err()
        );
        assert!(save(&path, "key", &[4; 32], &[row(1, "gain", 4)]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn linked_cache_path_is_refused() {
        use std::os::unix::fs::symlink;
        let directory = TestDir::new();
        let target = directory.0.join("target.json");
        fs::write(&target, b"{}").unwrap();
        let linked = directory.0.join("linked.json");
        symlink(&target, &linked).unwrap();
        assert!(load(&linked, "key").is_err());
        assert!(save(&linked, "key", &[4; 32], &[row(1, "gain", 0)]).is_err());
    }
}
