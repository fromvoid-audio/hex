fn main() { println!("cargo:rerun-if-changed=build.rs"); let value = if std::path::Path::new("empty-flag").is_dir() { 17 } else { 7 }; println!("cargo:rustc-env=DIRECTORY_VALUE={}", value); }
