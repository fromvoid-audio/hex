from pathlib import Path
import runpy
runpy.run_path('/tmp/2791-repair.py')
p=Path('crates/hex-ui/src/editor/browser/samples.rs');s=p.read_text();old='''        let remap = matches!(
            self.browser.section,
            BrowserSection::All | BrowserSection::Samples | BrowserSection::Place(_)
        );''';new='''        let remap = !self.browser_place_unavailable()
            && matches!(
                self.browser.section,
                BrowserSection::All | BrowserSection::Samples | BrowserSection::Place(_)
            );''';assert old in s;s=s.replace(old,new);p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser_ui_test.rs');s=p.read_text();old='''        let filtered = view.browser_visible_entries(cx);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, other);
        assert_eq!(view.browser.directory, second.0);
    });
    std::fs::remove_file(&other).unwrap();''';new='''        assert!(view.browser_visible_entries(cx).is_empty());
        assert!(view.browser.loading);
        assert_eq!(view.browser.directory, second.0);
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let filtered = view.browser_visible_entries(cx);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, other);
    });
    std::fs::remove_file(&other).unwrap();''';assert old in s;s=s.replace(old,new)+Path('/tmp/2791-ui-regressions-v2.rs').read_text();p.write_text(s)
p=Path('crates/hex-library/src/catalog/tests.rs');p.write_text(p.read_text()+Path('/tmp/2791-library-regressions.rs').read_text())
p=Path('crates/hex-ui/src/editor/browser_ui_test.rs');s=p.read_text();old='''                view.browser.directory = directory.0.clone();
                view.browser.listing = Some(DirectoryListing {''';new='''                view.browser.directory = directory.0.clone();
                // This fixture installs a validated snapshot, replacing startup work.
                view.browser.loading = false;
                view.browser.error = None;
                view.browser.listing = Some(DirectoryListing {''';assert old in s;s=s.replace(old,new);p.write_text(s)
