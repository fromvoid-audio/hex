import subprocess,pathlib,json,re
checks=[]
p=pathlib.Path('/tmp/414-final-ui-library.log');s=p.read_text();assert re.search(r'test result: ok\. 772 passed; 0 failed; 3 ignored;',s)
checks.append(dict(name='ui-library',command=['cargo','test','-p','hex-ui','--features','ui-test-support','--locked','--lib','--','--nocapture'],exit=0,log=str(p)))
for name,cmd in [
 ('clippy-default',['cargo','clippy','-p','hex-ui','--all-targets','--locked','--','-D','warnings']),
 ('clippy-test-support',['cargo','clippy','-p','hex-ui','--all-targets','--features','ui-test-support','--locked','--','-D','warnings']),
 ('architecture',['python3','scripts/check-architecture.py']),('gpui',['python3','scripts/check-gpui-patch.py']),('style',['python3','scripts/check-ui-style.py']),('fmt',['cargo','fmt','--all','--','--check']),('diff',['git','diff','--check'])]:
 log=f'/tmp/414-final-{name}.log'
 with open(log,'w') as f:result=subprocess.run(cmd,stdout=f,stderr=subprocess.STDOUT)
 checks.append(dict(name=name,command=cmd,exit=result.returncode,log=log));pathlib.Path('/tmp/414-final-checks.json').write_text(json.dumps(checks,indent=2)+'\n');print(name,result.returncode,flush=True)
 if result.returncode:raise SystemExit(result.returncode)
