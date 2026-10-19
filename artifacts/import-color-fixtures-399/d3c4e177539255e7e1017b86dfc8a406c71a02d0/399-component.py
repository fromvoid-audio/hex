import importlib.util,sys,pathlib,json
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_local_ci',root/'scripts/local-ci.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m)
name=sys.argv[1];commands=json.loads(sys.argv[2]);runner=m.LocalCiRunner(root,mode='component',run_id='track-color-fixtures-399-'+name,steps=tuple(m.Step(label,tuple(args)) for label,args in commands))
result=runner.run();print('COMPONENT_ONLY_SUMMARY',runner.summary_path);raise SystemExit(result)
