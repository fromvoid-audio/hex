import importlib.util,pathlib,sys
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_local_ci',root/'scripts/local-ci.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m)
steps=(m.Step('ui-default-strict',('cargo','clippy','-p','hex-ui','--all-targets','--locked','--','-D','warnings'),uses_cargo_target=True),)
raise SystemExit(m.LocalCiRunner(root,mode='component',run_id='329-command-motion-default-strict',steps=steps).run())
