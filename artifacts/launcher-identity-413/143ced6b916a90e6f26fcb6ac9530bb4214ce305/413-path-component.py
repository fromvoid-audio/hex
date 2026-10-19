import importlib.util,sys,pathlib
root=pathlib.Path('/workspace/hex')
spec=importlib.util.spec_from_file_location('hex_local_ci',root/'scripts/local-ci.py')
m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m)
steps=[('launcher-tests',('python3','-m','unittest','discover','-s','scripts/tests','-p','test_launch*.py','-v')),('python39-grammar',('python3','/tmp/413-grammar-check.py')),('scope-proof',('python3','/tmp/413-scope-proof.py'))]
for name,script in [('architecture','check-architecture.py'),('gpui-patch','check-gpui-patch.py'),('dependency-patches','check-dependency-patches.py'),('ui-style','check-ui-style.py'),('device-layout','check-device-layout.py')]:
 steps.append((name,('python3','scripts/'+script)))
steps += [('format',('cargo','fmt','--all','--check')),('diff',('git','diff','--check'))]
runner=m.LocalCiRunner(root,mode='component',run_id='launcher-path-413-143ced6b',steps=tuple(m.Step(name,args) for name,args in steps))
result=runner.run();print('COMPONENT_ONLY_SUMMARY',runner.summary_path);raise SystemExit(result)
