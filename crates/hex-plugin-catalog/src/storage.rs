//! Atomic current-format publication; invalid existing catalogs are preserved.
use crate::{Catalog, MAX_CATALOG_BYTES};
use hex_plugin_scan::macos::identity::read_file;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

fn plain(path: &Path) -> Result<(), String> {
    for member in path.ancestors() {
        match fs::symlink_metadata(member) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("linked plugin catalog path refused".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("plugin catalog metadata failed: {e}")),
        }
    }
    Ok(())
}
impl Catalog {
    /// Background control worker only. Missing is empty; malformed is an error.
    pub fn load(path: &Path) -> Result<Self, String> {
        plain(path)?;
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("plugin catalog unavailable: {e}")),
            Ok(_) => {}
        }
        let data = read_file(path, MAX_CATALOG_BYTES as u64)?;
        let mut catalog: Self = serde_json::from_slice(&data)
            .map_err(|_| "invalid plugin catalog; existing file preserved")?;
        // Migration is in memory; the file changes only through a later update.
        catalog.migrate()?;
        catalog.validate()?;
        Ok(catalog)
    }
}

pub struct Publication {
    pub catalog: Catalog,
    /// A rename may have succeeded even if the final directory sync failed.
    pub warning: Option<String>,
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// One nonblocking writer reservation, fresh read, mutation and atomic replace.
/// The closure runs under the catalog lock, never while foreign code executes.
pub fn update(
    path: &Path,
    change: impl FnOnce(&mut Catalog) -> Result<(), String>,
) -> Result<Publication, String> {
    plain(path)?;
    let directory = path.parent().ok_or("plugin catalog parent missing")?;
    fs::create_dir_all(directory).map_err(|e| format!("plugin catalog folder failed: {e}"))?;
    let lock_path = path.with_extension("lock");
    plain(&lock_path)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| format!("plugin catalog lock unavailable: {e}"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|_| "another window is updating the plugin catalog; retry")?;
    let mut catalog = Catalog::load(path)?;
    change(&mut catalog)?;
    catalog.validate()?;
    let data = serde_json::to_vec(&catalog).map_err(|_| "plugin catalog encoding failed")?;
    if data.len() > MAX_CATALOG_BYTES {
        return Err("plugin catalog byte bound exceeded".into());
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("plugin catalog filename invalid")?;
    let mut pending = None;
    for _ in 0..8 {
        let temporary = directory.join(format!(
            ".{name}.{}-{}.pending",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => {
                pending = Some((Temporary(temporary), file));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("plugin catalog temporary file failed: {e}")),
        }
    }
    let (temporary, mut file) = pending.ok_or("plugin catalog temporary file capacity exceeded")?;
    file.write_all(&data)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("plugin catalog write failed; previous file kept: {e}"))?;
    drop(file);
    fs::rename(&temporary.0, path)
        .map_err(|e| format!("plugin catalog publication failed; previous file kept: {e}"))?;
    let warning = File::open(directory)
        .and_then(|f| f.sync_all())
        .err()
        .map(|e| format!("plugin catalog replaced; durability unconfirmed: {e}"));
    Ok(Publication { catalog, warning })
}
