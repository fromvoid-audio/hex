import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
def git(*args,env=None):return subprocess.check_output(['git',*args],env=env)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source='1431195be7f48886a294e82e0bb1ddb0ccb83da5';base='07799e615c95a00864e688b2689bf90cd8327957';ref='evidence/310-quota-1431195b-20261019';dest=root/'artifacts/310-sealed-evidence'/source
assert git('rev-parse','HEAD').decode().strip()==source and not git('status','--porcelain') and not dest.exists()
p=root/'artifacts/local-ci/310-quota-final-v2/summary.json';s=json.loads(p.read_text());assert s['exit_code']==0 and not s['stale'] and s['start_tree']==s['end_tree'] and not s['start_tree']['dirty'] and s['start_tree']['commit']==source
dest.mkdir(parents=True)
for stage in ['baseline','after','differential','functional','final','final-v2']:
 shutil.copytree(root/('artifacts/local-ci/310-quota-'+stage),dest/'runs'/stage)
 shutil.copy2(root/('artifacts/310-'+stage+'-run.log'),dest/(stage+'-outer.log'))
for stage in ['baseline','after','differential','final','bound-red']:
 shutil.copytree(root/('artifacts/310-metadata-harness/'+stage)/'src',dest/'harness'/stage/'src')
 for name in ['Cargo.toml','Cargo.lock','EXTRACTION.json']:
  q=root/('artifacts/310-metadata-harness/'+stage)/name
  if q.exists():shutil.copy2(q,dest/'harness'/stage/name)
shutil.copy2(root/'artifacts/310-original-work-bound-red.log',dest/'actual-original-work-bound-red.log')
shutil.copy2(root/'artifacts/310-extraction-proof.json',dest/'extraction-proof.json')
files=git('diff','--name-only',base,source).decode().splitlines();assert files==['crates/hex-ui/src/editor/adaptive_grid_density_tests.rs','crates/hex-ui/src/editor/musical_time.rs','docs/adaptive-grid-menu.md'];ids={}
for name in files:
 ids[name]={}
 for label,rev in [('before',base),('after',source)]:
  b=git('show',rev+':'+name);q=dest/'source'/label/name;q.parent.mkdir(parents=True,exist_ok=True);q.write_bytes(b);ids[name][label]={'git_blob':git('rev-parse',rev+':'+name).decode().strip(),'sha256':hashlib.sha256(b).hexdigest()}
 assert (root/name).read_bytes()==git('show',source+':'+name)
assert git('show',source+':'+files[0]).startswith(git('show',base+':'+files[0]))
proof={'only_three_declared_files_changed':files,'all_original_density_test_bytes_preserved':True,'guard_runner_sha256':sha(root/'artifacts/310-approved-guard-runner.py'),'guard_runner_git_blob':git('rev-parse','688ce8d368d618458e1cd9f0fb7011fa92576c96:scripts/local-ci.py').decode().strip()}
assert (root/'artifacts/310-approved-guard-runner.py').read_bytes()==git('show','688ce8d368d618458e1cd9f0fb7011fa92576c96:scripts/local-ci.py')
for name in ['AGENTS.md','scripts/local-ci.py','scripts/check-ui-style.py','scripts/check-gpui-patch.py','Cargo.toml','Cargo.lock','crates/hex-ui/src/editor/trim.rs','crates/hex-ui/src/editor/controller.rs','crates/hex-ui/src/editor/context_menu.rs','crates/hex-ui/src/editor/adaptive_grid_menu.rs','crates/hex-ui/src/editor/adaptive_grid_menu_tests.rs']:
 assert git('show',base+':'+name)==git('show',source+':'+name);proof[name]=hashlib.sha256(git('show',source+':'+name)).hexdigest()
(dest/'source-identities.json').write_text(json.dumps(ids,indent=2)+'\n');(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(dest/'source.patch').write_bytes(git('diff','--binary',base,source));(dest/'source-history.txt').write_bytes(git('log','--format=fuller',base+'..'+source))
# Robust work counts vs noisy single cloud timing observations.
import re
records={}
for stage,folder in [('baseline','baseline'),('final','final-v2')]:
 logs=sorted((dest/'runs'/folder).glob('02-*.log' if stage=='final' else '01-*.log'));assert len(logs)==1;txt=logs[0].read_text()
 records[stage]=[dict(zip(['count','density','operation','step','base','shares','demand','ns'],m.groups())) for m in re.finditer(r'GRID_WORK count=(\d+) density=(\w+) operation=(\w+) step=(\d+) base=(\d+) shares=(\d+) demand=(\d+) ns=(\d+)',txt)];assert len(records[stage])==72
(dest/'actual-work-matrix.json').write_text(json.dumps(records,indent=2)+'\n')
final={'source':source,'tree':git('rev-parse',source+'^{tree}').decode().strip(),'base':base,'summary_sha256':sha(p),'seconds':s['duration_seconds'],'source_content_sha256':s['start_tree']['content_sha256']}
(dest/'HANDOFF.md').write_text(Path('/tmp/310-handoff.md').read_text()+'\n## Final clean exact receipt\n\n'+json.dumps(final,indent=2)+'\n')
for name in ['/tmp/310-instrument.py','/tmp/310-repair.py','/tmp/310-add-oracles.py','/tmp/310-metadata-harness.py','/tmp/310-component.py','/tmp/310-guarded-component.py','/tmp/310-bound-red.py','/tmp/seal-310.py','/workspace/hex/artifacts/310-approved-guard-runner.py']:
 q=dest/'orchestration'/Path(name).name;q.parent.mkdir(exist_ok=True);shutil.copy2(name,q)
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()};(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n');archive=dest/'hex310-quota-1431195b-evidence.tar.gz'
with tarfile.open(archive,'w:gz') as tf:
 for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
 assert len(tf.getmembers())==len(manifest)+1
 for m in tf.getmembers():
  assert m.isfile() and not m.name.startswith('/') and '..' not in Path(m.name).parts
  if m.name in manifest:assert hashlib.sha256(tf.extractfile(m).read()).hexdigest()==manifest[m.name]['sha256']
with tempfile.TemporaryDirectory() as d:
 env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')};subprocess.run(['git','read-tree','--empty'],env=env,check=True);subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True);tree=git('write-tree',env=env).decode().strip();commit=git('commit-tree',tree,'-m','Seal PR310 per-operation quota repair and source-bound evidence',env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={**final,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1};(root/'artifacts/310-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:assert git('show',commit+':'+str((dest/name).relative_to(root)))==(dest/name).read_bytes()
print(json.dumps(state,indent=2),flush=True)
