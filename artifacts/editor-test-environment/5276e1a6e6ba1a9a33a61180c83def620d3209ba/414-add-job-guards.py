from pathlib import Path

def one_job(path,name,operation):
 p=Path(path);s=p.read_text();a=s.index('fn '+name+'(');b=s.find('\n    fn ',a+5)
 # Public adjacent methods also terminate the chunk; locate next method at any visibility.
 import re
 m=re.search(r'\n    (?:pub\([^)]*\) )?fn ',s[a+5:])
 if m:b=a+5+m.start()
 if b<0:b=len(s)
 chunk=s[a:b]
 marker='        let this = cx.entity().downgrade();'
 assert chunk.count(marker)==1,(name,'outer')
 chunk=chunk.replace(marker,'        #[cfg(test)]\n        let test_environment = std::sync::Arc::clone(&self.test_environment);\n'+marker)
 needle='async move { '+operation+' }'
 assert chunk.count(needle)==1,(name,operation)
 chunk=chunk.replace(needle,'async move {\n                        #[cfg(test)]\n                        let _test_environment = test_environment;\n                        '+operation+'\n                    }')
 p.write_text(s[:a]+chunk+s[b:])
root='crates/hex-ui/src/editor/'
for name,op in [('start_browser_places_load','hex_preferences::load(&path)'),('start_browser_places_save','hex_preferences::save(&path, &places)'),('start_browser_sample_playback','hex_session::service::prepare_sample_preview(path, sample_rate)'),('start_browser_audio_preview','hex_session::service::prepare_audio_preview(path, sample_rate)'),('start_browser_scan','scan_directory(path_for_scan)')]:one_job(root+'browser.rs',name,op)
one_job(root+'browser/tree.rs','start_browser_tree_worker','scan_directory(path)')
for file,items in [('recent_projects.rs',[('start_recent_projects_load','hex_preferences::load_recent_projects(&path)'),('start_recent_projects_save','hex_preferences::save_recent_projects(&path, &projects)')]),('theme_settings.rs',[('start_theme_preferences_load','hex_preferences::load_theme_preferences(&path)'),('start_theme_save','hex_preferences::save_theme_preferences(&path, &colors)')])]:
 for name,op in items:one_job(root+file,name,op)
p=Path(root+'browser/samples.rs');s=p.read_text();a=s.index('    pub(super) fn refresh_sample_catalog(');chunk=s[a:];marker='        let this = cx.entity().downgrade();';assert chunk.count(marker)==1;chunk=chunk.replace(marker,'        #[cfg(test)]\n        let test_environment = Arc::clone(&self.test_environment);\n'+marker)
chunk=chunk.replace('                let path = database.clone();','                let path = database.clone();\n                #[cfg(test)]\n                let load_environment = Arc::clone(&test_environment);')
chunk=chunk.replace('async move { SampleCatalog::load(&path) }','async move {\n                        #[cfg(test)]\n                        let _test_environment = load_environment;\n                        SampleCatalog::load(&path)\n                    }')
chunk=chunk.replace('''                    let scan = scan_samples(&scan_roots);
                    let saved = scan.catalog.save(&database);
                    (scan, saved)''','''                    #[cfg(test)]
                    let test_environment = test_environment;
                    let scan = scan_samples(&scan_roots);
                    #[cfg(test)]
                    let gate = test_environment.before_catalog_save().await;
                    let saved = scan.catalog.save(&database);
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        gate.did_save(saved.is_ok(), test_environment.root().is_dir());
                    }
                    (scan, saved)''')
p.write_text(s[:a]+chunk)
# Accessors are test-only, not part of production State visibility.
for name in ['recent_projects.rs','theme_settings.rs']:
 p=Path(root+name);s=p.read_text().replace('#[cfg(test)]\n    pub(super) fn test_path', '#[cfg(all(test, feature = "ui-test-support"))]\n    pub(super) fn test_path');p.write_text(s)
