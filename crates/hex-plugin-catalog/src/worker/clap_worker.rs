//! Bounded CLAP folder discovery using the shared [`crate::roots::enumerate_for`]
//! enumerator and [`crate::format::CatalogFormatBackend`], updating
//! [`Cancellation::current_module()`] for every inspected bundle.
use super::*;
use std::collections::BTreeSet;

/// Explicit standard-folder CLAP discovery (`~/Library/Audio/Plug-Ins/CLAP` and
/// `/Library/Audio/Plug-Ins/CLAP`) merged into the recoverable plugin catalog.
pub fn scan_clap_standard(catalog_path: &Path, token: Token) -> Update {
    match crate::format::backend(crate::PluginFormat::Clap).standard_roots() {
        Ok(roots) => scan_clap_folders(catalog_path, &roots, token),
        Err(status) => Update {
            catalog: None,
            diagnostics: None,
            status,
        },
    }
}

/// Bounded user-folder CLAP discovery merged atomically into the plugin catalog
/// using the shared `roots::enumerate_for` enumerator and cancellation/publication
/// token boundary.
pub fn scan_clap_folders(catalog_path: &Path, roots: &[PathBuf], token: Token) -> Update {
    scan_clap_folders_with_inspector(
        catalog_path,
        roots,
        token,
        hex_clap_scan::user_folder::inspect_clap_bundle,
    )
}

fn scan_clap_folders_with_inspector<F>(
    catalog_path: &Path,
    roots: &[PathBuf],
    token: Token,
    mut inspect: F,
) -> Update
where
    F: FnMut(&Path) -> Result<hex_clap_scan::user_folder::ClapBundleIdentity, String>,
{
    type StagedClapScan = (
        Vec<(
            PathBuf,
            Result<hex_clap_scan::user_folder::ClapBundleIdentity, String>,
        )>,
        usize,
        usize,
        usize,
    );

    let staged_result = (|| -> Result<StagedClapScan, String> {
        let staged = Catalog::load(catalog_path)?;
        let enumeration = crate::format::backend(crate::PluginFormat::Clap)
            .enumerate(roots, &mut || token.cancelled())?;
        let mut all_paths: BTreeSet<PathBuf> = staged
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .chain(staged.clap_entries.iter().map(|entry| entry.path.clone()))
            .collect();
        for bundle in &enumeration.bundles {
            all_paths.insert(bundle.clone());
        }
        if all_paths.len() > crate::MAX_MODULES {
            return Err("plugin catalog module capacity reached before clap scanning".into());
        }
        let mut results = Vec::with_capacity(enumeration.bundles.len());
        for bundle in enumeration.bundles {
            if token.cancelled() {
                return Err("clap folder scan cancelled · catalog unchanged".into());
            }
            let _active = CurrentModuleGuard::enter(&token, &bundle);
            let outcome = inspect(&bundle);
            results.push((bundle, outcome));
        }
        let discovered = results.iter().filter(|(_, res)| res.is_ok()).count();
        let failed = results.len().saturating_sub(discovered);
        Ok((results, discovered, failed, enumeration.skipped_links))
    })();

    let (results, discovered, failed, skipped_links) = match staged_result {
        Ok(value) => value,
        Err(status) => {
            return Update {
                catalog: None,
                diagnostics: None,
                status,
            };
        }
    };

    match token.begin_publication() {
        Ok(true) => {}
        Ok(false) => {
            return Update {
                catalog: None,
                diagnostics: None,
                status: "clap folder scan cancelled · catalog unchanged".into(),
            };
        }
        Err(status) => {
            return Update {
                catalog: None,
                diagnostics: None,
                status,
            };
        }
    }

    let status = format!(
        "clap scan complete · {discovered} discovered · {failed} failed · {skipped_links} linked entries skipped"
    );

    if results.is_empty() {
        return match Catalog::load(catalog_path) {
            Ok(catalog) => Update {
                catalog: Some(catalog),
                diagnostics: None,
                status,
            },
            Err(error) => Update {
                catalog: None,
                diagnostics: None,
                status: format!("clap folder scan catalog reload failed: {error}"),
            },
        };
    }

    match crate::storage::update(catalog_path, |catalog| {
        for (path, outcome) in &results {
            match outcome {
                Ok(identity) => {
                    catalog.record_clap(path.clone(), Ok((identity.clone(), Vec::new())))?
                }
                Err(error) => catalog.record_clap(path.clone(), Err(error.clone()))?,
            }
        }
        Ok(())
    }) {
        Ok(publication) => Update {
            catalog: Some(publication.catalog),
            diagnostics: None,
            status,
        },
        Err(error) => Update {
            catalog: None,
            diagnostics: None,
            status: format!("{status} · catalog update failed: {error}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn clap_folder_scan_uses_roots_enumerator_and_updates_current_module() {
        let base = std::env::temp_dir().join(format!(
            "hex-clap-worker-unit-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        let root = base.canonicalize().unwrap();
        let bundle = root.join("Demo.clap");
        fs::create_dir_all(&bundle).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&bundle, root.join("Alias.clap")).unwrap();
        let catalog_path = root.join("catalog.json");

        let cancellation = Cancellation::default();
        assert_eq!(cancellation.current_module(), None);
        let mut observed_current = Vec::new();
        let update = scan_clap_folders_with_inspector(
            &catalog_path,
            std::slice::from_ref(&root),
            cancellation.token(),
            |path| {
                observed_current.push(cancellation.current_module());
                Err(format!("synthetic inspect failure for {}", path.display()))
            },
        );
        assert_eq!(observed_current, vec![Some(bundle)]);
        assert_eq!(cancellation.current_module(), None);
        assert!(update.catalog.is_some());
        assert!(update.status.contains("0 discovered · 1 failed"));
        #[cfg(unix)]
        assert!(update.status.contains("1 linked entries skipped"));
        let _ = fs::remove_dir_all(&root);
    }
}
