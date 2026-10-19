from pathlib import Path
import json,hashlib
root=Path('/workspace/hex');rows=[]
for summary in sorted((root/'artifacts/local-ci').glob('474-warnings-*/summary.json')):
 s=json.loads(summary.read_text());files=[]
 for p in sorted(summary.parent.glob('*.log')):
  lines=p.read_text(errors='replace').splitlines();warnings=[line for line in lines if line.startswith('warning:')]
  files.append({'log':str(p.relative_to(root)),'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'warning_lines':warnings,'errors':[x for x in lines if x.startswith('error:') or x.startswith('error[')]})
 rows.append({'summary':str(summary.relative_to(root)),'summary_sha256':hashlib.sha256(summary.read_bytes()).hexdigest(),'status':s['status'],'source':s.get('start_tree'),'logs':files})
(root/'artifacts/474-warning-inventory.json').write_text(json.dumps(rows,indent=2)+'\n')
