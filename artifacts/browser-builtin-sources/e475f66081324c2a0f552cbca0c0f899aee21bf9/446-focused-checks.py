import subprocess,json
from pathlib import Path
commands=[
 ('preferences',['cargo','test','-p','hex-preferences','--locked','--','--nocapture']),
 ('browser',['cargo','test','-p','hex-ui','--features','ui-test-support','--locked','--lib','editor::browser::','--','--nocapture']),
 ('context-menu',['cargo','test','-p','hex-ui','--features','ui-test-support','--locked','--lib','context_menu','--','--nocapture']),
 ('clippy',['cargo','clippy','-p','hex-ui','-p','hex-preferences','--all-targets','--features','hex-ui/ui-test-support','--locked','--','-D','warnings']),
 ('architecture',['python3','scripts/check-architecture.py']),
 ('gpui',['python3','scripts/check-gpui-patch.py']),
 ('style',['python3','scripts/check-ui-style.py']),
 ('fmt',['cargo','fmt','--all','--','--check']),
 ('diff',['git','diff','--check']),
]
results=[]
for name,command in commands:
 log=f'/tmp/446-final-{name}.log'
 with open(log,'w') as f:r=subprocess.run(command,stdout=f,stderr=subprocess.STDOUT)
 results.append(dict(name=name,command=command,exit=r.returncode,log=log))
 Path('/tmp/446-final-checks.json').write_text(json.dumps(results,indent=2)+'\n')
 print(name,r.returncode,flush=True)
 if r.returncode:break
