//! Bounded helper traversal: one fixed-capacity path worklist, bounded relative
//! paths, and one file snapshot at a time. OS enumeration and BTreeMap node
//! implementation costs remain distinct from these listed DATA bounds.
use super::*;

pub(super) const MAX_MEMBERS: usize = 256;
const MAX_DEPTH: usize = 32;

fn relative_member<'a>(root: &Path, path: &'a Path) -> Result<&'a str, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "helper root mismatch")?
        .to_str()
        .ok_or("helper member encoding refused")?;
    if relative.is_empty() || relative.len() > MAX_PATH_BYTES {
        return Err("helper relative path bound".into());
    }
    Ok(relative)
}

pub(super) fn inventory(root: &Path, app: &str) -> Result<BTreeMap<String, String>, String> {
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(MAX_MEMBERS)
        .map_err(|_| "helper worklist allocation refused")?;
    if pending.capacity() != MAX_MEMBERS {
        return Err("helper worklist allocation bound differs".into());
    }
    pending.push((root.join(app), 0_usize));
    let mut files = BTreeMap::new();
    let mut discovered = 0_usize;
    let mut total = 0_u64;
    while let Some((path, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            return Err("helper bundle member/depth bound".into());
        }
        let relative = relative_member(root, &path)?;
        let meta =
            fs::symlink_metadata(&path).map_err(|e| format!("helper member missing: {e}"))?;
        if meta.file_type().is_symlink() {
            return Err("linked helper member refused".into());
        }
        if meta.is_dir() {
            for member in
                fs::read_dir(&path).map_err(|e| format!("helper enumeration failed: {e}"))?
            {
                let member = member.map_err(|e| format!("helper enumeration failed: {e}"))?;
                // Count on discovery, before constructing another queued path.
                // No push can trigger a worklist growth or exceed total work.
                if discovered == MAX_MEMBERS || pending.len() == MAX_MEMBERS {
                    return Err("helper bundle member/depth bound".into());
                }
                let child = member.path();
                relative_member(root, &child)?;
                discovered += 1;
                pending.push((child, depth + 1));
            }
        } else if meta.is_file() {
            let bytes = read_file(&path, MAX_BINARY)?;
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or("helper size overflow")?;
            if total > 32 * 1024 * 1024 {
                return Err("helper bundle byte bound".into());
            }
            files.insert(relative.to_owned(), hash(&bytes));
        } else {
            return Err("nonregular helper member refused".into());
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests;
