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
}

#[cfg(test)]
mod tests {
    use super::*;
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
