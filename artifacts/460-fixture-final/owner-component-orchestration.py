import importlib.util,pathlib,sys
root=pathlib.Path('/workspace/hex');path=pathlib.Path('/tmp/hex460-current-guard/local-ci.py')
spec=importlib.util.spec_from_file_location('hex_current_local_ci',path);m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m);m.ROOT=root
if sys.argv[1]=='preflight':
 steps=(*m.QUICK_STEPS,m.Step('diff',('git','diff','--check')))
else:
 steps=(m.Step('arrow-fixture-tests',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','browser_arrows','--','--nocapture'),uses_cargo_target=True,requires_tests=True),m.Step('environment-custody-tests',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','browser_test_environment','--','--nocapture'),uses_cargo_target=True,requires_tests=True),m.Step('ui-feature-strict',('cargo','clippy','-p','hex-ui','--features','ui-test-support','--all-targets','--locked','--','-D','warnings'),uses_cargo_target=True),m.Step('format',('cargo','fmt','--all','--check')))
r=m.LocalCiRunner(root,mode='component',run_id='460-fixture-'+sys.argv[1]+(('-'+sys.argv[2]) if len(sys.argv)>2 else ''),steps=steps)
raise SystemExit(r.run())
