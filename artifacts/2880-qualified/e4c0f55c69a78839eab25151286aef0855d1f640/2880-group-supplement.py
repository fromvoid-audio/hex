import importlib.util,sys,pathlib
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_local_ci',root/'artifacts/310-approved-guard-runner.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m);m.ROOT=root
steps=(m.Step('group drag corrected module',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','--lib','editor::drag::group_drag_ui_test::','--','--nocapture'),uses_cargo_target=True),)
r=m.LocalCiRunner(root,mode='component',run_id=sys.argv[1],steps=steps);code=r.run();print('COMPONENT_ONLY_SUMMARY',r.summary_path);raise SystemExit(code)
