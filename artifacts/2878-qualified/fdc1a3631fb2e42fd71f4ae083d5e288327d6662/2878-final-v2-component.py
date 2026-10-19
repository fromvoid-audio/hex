import importlib.util,sys,pathlib
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_local_ci',root/'artifacts/310-approved-guard-runner.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m);m.ROOT=root
steps=(
m.Step('folder bookmarks actual pointer persistence and custody',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','--lib','folder_places_ui_test::','--','--nocapture'),uses_cargo_target=True),
m.Step('existing browser navigation tree icons and audition',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','--lib','browser::ui_test::','--','--nocapture'),uses_cargo_target=True),
m.Step('existing browser focus controls',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','--lib','explicit_focus::','--','--nocapture'),uses_cargo_target=True),
m.Step('strict UI default',('cargo','clippy','-p','hex-ui','--all-targets','--locked','--','-D','warnings'),uses_cargo_target=True),
m.Step('strict UI feature',('cargo','clippy','-p','hex-ui','--all-targets','--features','ui-test-support','--locked','--','-D','warnings'),uses_cargo_target=True),
m.Step('architecture',('python3','scripts/check-architecture.py')),
m.Step('UI style',('python3','scripts/check-ui-style.py')),
m.Step('GPUI patch',('python3','scripts/check-gpui-patch.py')),
m.Step('dependency provenance',('python3','scripts/check-dependency-patches.py')),
m.Step('device layout',('python3','scripts/check-device-layout.py')),
m.Step('format',('cargo','fmt','--all','--check')),
m.Step('diff',('git','diff','--check')))
r=m.LocalCiRunner(root,mode='component',run_id=sys.argv[1],steps=steps);code=r.run();print('COMPONENT_ONLY_SUMMARY',r.summary_path);raise SystemExit(code)
