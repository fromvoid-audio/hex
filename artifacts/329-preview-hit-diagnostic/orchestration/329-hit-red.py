import importlib.util,pathlib,sys
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_local_ci',root/'scripts/local-ci.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m)
steps=(m.Step('held-preview-narrow-controls',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','review_ui_tests','--','--nocapture'),uses_cargo_target=True,requires_tests=True),)
raise SystemExit(m.LocalCiRunner(root,mode='component',run_id='329-held-narrow-red',steps=steps).run())
