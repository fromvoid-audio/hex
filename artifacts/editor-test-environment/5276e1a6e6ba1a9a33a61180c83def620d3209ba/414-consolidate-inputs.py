from pathlib import Path
p=Path('crates/hex-ui/src/editor/test_environment.rs');s=p.read_text();marker='static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);';s=s.replace(marker,'''/// One test-only constructor input boundary; explicit fixture paths are caller-owned.
#[derive(Default)]
pub(super) struct Inputs {
    pub(super) directory: Option<PathBuf>,
    pub(super) home: Option<PathBuf>,
    pub(super) defaults: [bool; 3],
}

'''+marker);p.write_text(s)
p=Path('crates/hex-ui/src/editor/mod.rs');s=p.read_text()
s=s.replace('''                None,
                None,
                [true; 3],''','''                test_environment::Inputs { defaults: [true; 3], ..Default::default() },''')
s=s.replace('''            None,
            None,
            [false, false, true],''','''            test_environment::Inputs { defaults: [false, false, true], ..Default::default() },''')
s=s.replace('''            Some(directory.join("samples")),
            None,
            [false; 3],''','''            test_environment::Inputs { directory: Some(directory.join("samples")), ..Default::default() },''')
s=s.replace('    #[cfg(any(not(test), feature = "ui-test-support"))]\n    fn new_with_browser_preferences_path(', '    fn new_with_browser_preferences_path(')
s=s.replace('''            #[cfg(test)]
            None,
            #[cfg(test)]
            None,
            #[cfg(test)]
            [false; 3],''','''            #[cfg(test)]
            test_environment::Inputs::default(),''')
s=s.replace('''        #[cfg(test)] test_browser_directory: Option<std::path::PathBuf>,
        #[cfg(test)] test_browser_home: Option<std::path::PathBuf>,
        #[cfg(test)] test_default_paths: [bool; 3],''','''        #[cfg(test)] test_paths: test_environment::Inputs,''')
s=s.replace('test_environment.resolve_paths(paths, test_default_paths)', 'test_environment.resolve_paths(paths, test_paths.defaults)')
s=s.replace('test_browser_home.unwrap_or_else', 'test_paths.home.unwrap_or_else')
s=s.replace('if let Some(directory) = test_browser_directory', 'if let Some(directory) = test_paths.directory')
assert 'test_browser_' not in s
p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser_ui_test.rs');s=p.read_text().replace('''                    None,
                    Some(home),
                    [false; 3],''','''                    crate::editor::test_environment::Inputs { home: Some(home), ..Default::default() },''');p.write_text(s)
