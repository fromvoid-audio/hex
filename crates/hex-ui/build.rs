//! Embed release version label and changelog bytes for `hex-ui`.
//! Source-revision identity is captured at `hex-desktop` so unrelated commits
//! and workspace edits never force a rebuild of `hex-ui`.

fn main() {
    println!("cargo:rerun-if-env-changed=HEX_RELEASE_CHANGELOG");
    println!("cargo:rerun-if-env-changed=HEX_RELEASE_LABEL");
    println!("cargo:rerun-if-env-changed=HEX_GUARDED_UI_ASSETS_SHA256");
    let version =
        std::env::var("HEX_RELEASE_LABEL").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned());
    println!("cargo:rustc-env=HEX_DISPLAY_VERSION={version}");
    let release_dataset = std::env::var_os("HEX_RELEASE_CHANGELOG");
    let dataset = release_dataset
        .as_deref()
        .map(std::path::Path::new)
        .unwrap_or_else(|| std::path::Path::new("../../changelog/generated.json"));
    if dataset.exists() {
        println!("cargo:rerun-if-changed={}", dataset.display());
    } else if let Some(parent) = dataset.parent()
        && parent.exists()
    {
        println!("cargo:rerun-if-changed={}", parent.display());
    }
    let bytes = match std::fs::read(dataset) {
        Ok(bytes) if bytes.len() <= 1024 * 1024 => bytes,
        Ok(_) => panic!("changelog exceeds the validated dataset limit"),
        Err(error) if release_dataset.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
            Vec::new()
        }
        Err(error) => panic!("cannot embed changelog: {error}"),
    };
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo output directory"));
    let embedded = output.join("changelog.json");
    if std::fs::read(&embedded).ok().as_deref() != Some(bytes.as_slice()) {
        std::fs::write(embedded, bytes).expect("embed changelog");
    }
}
