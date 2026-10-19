import importlib.util,sys,pathlib,json
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_local_ci',root/'artifacts/310-approved-guard-runner.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m);m.ROOT=root
steps=tuple(m.Step(label,tuple(argv),uses_cargo_target=owned) for label,argv,owned in json.loads(sys.argv[2]))
r=m.LocalCiRunner(root,mode='component',run_id='463-resize-pan-'+sys.argv[1],steps=steps)
result=r.run();print('COMPONENT_ONLY_SUMMARY',r.summary_path);raise SystemExit(result)
