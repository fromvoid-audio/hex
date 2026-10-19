import importlib.util,pathlib,sys
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_local_ci',root/'scripts/local-ci.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m)
steps=(m.Step('command-vertical-motion',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','command_vertical_motion','--','--nocapture'),uses_cargo_target=True,requires_tests=True),)
r=m.LocalCiRunner(root,mode='component',run_id='329-command-motion-red',steps=steps)
raise SystemExit(r.run())
