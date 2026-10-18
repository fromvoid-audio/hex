import subprocess,pathlib,json,re
checks=[]
p=pathlib.Path('/tmp/444-final-browser.log');s=p.read_text()
match=re.search(r'test result: ok\. (\d+) passed; 0 failed;',s);assert match and int(match[1])>0
checks.append(dict(name='browser',command=['cargo','test','-p','hex-ui','--features','ui-test-support','--locked','browser','--','--nocapture'],exit=0,log=str(p)))
for name,cmd in [
 ('clippy',['cargo','clippy','-p','hex-ui','--all-targets','--features','ui-test-support','--locked','--','-D','warnings']),
 ('architecture',['python3','scripts/check-architecture.py']),('gpui',['python3','scripts/check-gpui-patch.py']),('style',['python3','scripts/check-ui-style.py']),('fmt',['cargo','fmt','--all','--','--check']),('diff',['git','diff','--check'])]:
 log=f'/tmp/444-final-{name}.log'
 with open(log,'w') as f:result=subprocess.run(cmd,stdout=f,stderr=subprocess.STDOUT)
 checks.append(dict(name=name,command=cmd,exit=result.returncode,log=log));pathlib.Path('/tmp/444-final-checks.json').write_text(json.dumps(checks,indent=2)+'\n');print(name,result.returncode,flush=True)
 if result.returncode:raise SystemExit(result.returncode)
