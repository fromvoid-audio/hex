//! Retained provenance metadata, separate from revalidation working buffers.
use super::*;

/// Actual path/string capacities and populated map key/value payloads. Build's
/// inline value belongs to its enclosing host cell. BTreeMap implementation/
/// allocator overhead is opaque, with at most one nonempty node per entry;
/// this is an obligation bound, not a node-layout or RSS estimate. Initial JSON
/// inspection and revalidation construction peaks are not included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildStorage {
    pub data_bytes: usize,
    pub inventory_entries: usize,
    pub inventory_node_lifetime_bound: usize,
}

impl Build {
    pub fn retained_storage(&self) -> Option<BuildStorage> {
        let mut data = self
            .helper
            .capacity()
            .checked_add(self.path.capacity())?
            .checked_add(self.root.capacity())?
            .checked_add(self.helper_sha256.capacity())?
            .checked_add(self.receipt_sha256.capacity())?;
        for (key, value) in &self.files {
            data = data
                .checked_add(std::mem::size_of_val(key))?
                .checked_add(std::mem::size_of_val(value))?
                .checked_add(key.capacity())?
                .checked_add(value.capacity())?;
        }
        Some(BuildStorage {
            data_bytes: data,
            inventory_entries: self.files.len(),
            inventory_node_lifetime_bound: self.files.len(),
        })
    }
}
impl InstanceBuild {
    pub fn retained_storage(&self) -> Option<BuildStorage> {
        self.0.retained_storage()
    }
    /// Copy provenance for a pending instance before native startup. Every
    /// path/string requests exactly its current length, without preserving
    /// spare capacity or predicting a derived Clone's allocation strategy.
    /// Known DATA cannot exceed `self.retained_storage()`. Map-node/allocator
    /// implementation overhead remains a separate bounded lifetime obligation.
    /// This performs no I/O, source verification or native admission.
    pub fn try_copy_on_control(&self) -> Result<Self, ValidationError> {
        let source = &self.0;
        if source.files.len() > MAX_MEMBERS {
            return Err("helper inventory bound".into());
        }
        let mut files = BTreeMap::new();
        for (key, value) in &source.files {
            files.insert(copy_text(key)?, copy_text(value)?);
        }
        Ok(Self(Build {
            helper: copy_path(&source.helper)?,
            helper_sha256: copy_text(&source.helper_sha256)?,
            receipt_sha256: copy_text(&source.receipt_sha256)?,
            path: copy_path(&source.path)?,
            root: copy_path(&source.root)?,
            files,
            role: source.role,
        }))
    }
}

fn copy_text(source: &str) -> Result<String, ValidationError> {
    let mut target = String::new();
    target
        .try_reserve_exact(source.len())
        .map_err(|_| "helper metadata allocation refused")?;
    if target.capacity() != source.len() {
        return Err("helper metadata allocation bound differs".into());
    }
    target.push_str(source);
    Ok(target)
}
fn copy_path(source: &Path) -> Result<PathBuf, ValidationError> {
    let mut target = PathBuf::new();
    target
        .try_reserve_exact(source.as_os_str().len())
        .map_err(|_| "helper path allocation refused")?;
    if target.capacity() != source.as_os_str().len() {
        return Err("helper path allocation bound differs".into());
    }
    target.push(source);
    if target.capacity() != source.as_os_str().len() {
        return Err("helper path copy exceeded its allocation bound".into());
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn instance_provenance_copy_preserves_values_without_spare_capacity() {
        let mut source = InstanceBuild(Build {
            helper: PathBuf::from("/original/é/helper"),
            helper_sha256: "a".repeat(64),
            receipt_sha256: "b".repeat(64),
            path: PathBuf::from("/original/receipt"),
            root: PathBuf::from("/original"),
            files: BTreeMap::from([("member".into(), "c".repeat(64))]),
            role: HelperRole::Instance,
        });
        source.0.helper.reserve(1024);
        source.0.helper_sha256.reserve(1024);
        source.0.files.get_mut("member").unwrap().reserve(1024);
        let original = source.retained_storage().unwrap();
        let copied = source.try_copy_on_control().unwrap();
        assert_eq!(copied.0.helper, source.0.helper);
        assert_eq!(copied.0.path, source.0.path);
        assert_eq!(copied.0.root, source.0.root);
        assert_eq!(copied.0.helper_sha256, source.0.helper_sha256);
        assert_eq!(copied.0.receipt_sha256, source.0.receipt_sha256);
        assert_eq!(copied.0.files, source.0.files);
        assert!(matches!(copied.0.role, HelperRole::Instance));
        for path in [&copied.0.helper, &copied.0.path, &copied.0.root] {
            assert_eq!(path.capacity(), path.as_os_str().len());
        }
        for text in [&copied.0.helper_sha256, &copied.0.receipt_sha256]
            .into_iter()
            .chain(copied.0.files.iter().flat_map(|(key, value)| [key, value]))
        {
            assert_eq!(text.capacity(), text.len());
        }
        let actual = copied.retained_storage().unwrap();
        assert!(actual.data_bytes < original.data_bytes);
        assert_eq!(
            actual.inventory_node_lifetime_bound,
            original.inventory_node_lifetime_bound
        );
        drop(copied);
        assert_eq!(source.retained_storage(), Some(original));
        for n in 0..MAX_MEMBERS {
            source.0.files.insert(format!("extra-{n}"), "d".repeat(64));
        }
        assert_eq!(source.0.files.len(), MAX_MEMBERS + 1);
        assert!(source.try_copy_on_control().is_err());
    }
    #[test]
    fn retained_metadata_counts_spare_path_and_digest_capacity() {
        let mut build = Build {
            helper: PathBuf::from("helper"),
            helper_sha256: "a".repeat(64),
            receipt_sha256: "b".repeat(64),
            path: PathBuf::from("receipt"),
            root: PathBuf::from("root"),
            role: HelperRole::Instance,
            files: BTreeMap::from([("helper".into(), "c".repeat(64))]),
        };
        let before = build.retained_storage().unwrap();
        assert_eq!(
            (
                before.inventory_entries,
                before.inventory_node_lifetime_bound
            ),
            (1, 1)
        );
        let capacity = build.helper.capacity();
        build.helper.reserve(333);
        let after_path = build.retained_storage().unwrap();
        assert_eq!(
            after_path.data_bytes - before.data_bytes,
            build.helper.capacity() - capacity
        );
        let value = build.files.get_mut("helper").unwrap();
        let capacity = value.capacity();
        value.reserve(255);
        let difference = value.capacity() - capacity;
        let after_digest = build.retained_storage().unwrap();
        assert_eq!(after_digest.data_bytes - after_path.data_bytes, difference);
        assert_eq!(InstanceBuild(build).retained_storage(), Some(after_digest));
    }
}
