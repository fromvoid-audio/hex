import importlib.util,pathlib,sys
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_local_ci',root/'scripts/local-ci.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m)
steps=(m.Step('fresh-midi-left',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','fresh_midi_left_extension','--','--nocapture'),uses_cargo_target=True,requires_tests=True),)
raise SystemExit(m.LocalCiRunner(root,mode='component',run_id='464-midi-left-red',steps=steps).run())
