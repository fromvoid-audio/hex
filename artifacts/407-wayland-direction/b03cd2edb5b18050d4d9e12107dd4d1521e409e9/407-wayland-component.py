import importlib.util,sys,pathlib,json
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_local_ci',root/'scripts/local-ci.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m)
runner=m.LocalCiRunner(root,mode='component',run_id='header-width-return-'+sys.argv[1],steps=tuple(m.Step(label,tuple(args),uses_cargo_target=args[:2]==["python3","/tmp/407-wayland-real-types.py"]) for label,args in json.loads(sys.argv[2])))
result=runner.run();print('COMPONENT_ONLY_SUMMARY',runner.summary_path);raise SystemExit(result)
