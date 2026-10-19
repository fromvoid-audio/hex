import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
def git(*args,env=None):return subprocess.check_output(['git',*args],env=env)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source='c0cbec36990546ceefbba22656d3819ddd143e5a';base='9c2f76ad19253c7a17c78053c1a5883d827c1ed9';ref='evidence/463-resize-pan-c0cbec36-20261019';dest=root/'artifacts/463-resize-pan-evidence'/source
assert git('rev-parse','HEAD').decode().strip()==source and not git('status','--porcelain') and not dest.exists()
p=root/'artifacts/local-ci/463-resize-pan-final/summary.json';s=json.loads(p.read_text());assert s['exit_code']==0 and not s['stale'] and s['start_tree']==s['end_tree'] and not s['start_tree']['dirty'] and s['start_tree']['commit']==source
receipt={'source':source,'seconds':s['duration_seconds'],'summary_sha256':sha(p),'content_sha256':s['start_tree']['content_sha256']}
dest.mkdir(parents=True)
for stage in ['red','green','controls','warp-fixture-trace','controls-v2','final']:
 shutil.copytree(root/('artifacts/local-ci/463-resize-pan-'+stage),dest/'runs'/stage)
 outer={'warp-fixture-trace':'artifacts/463-warp-fixture-trace-run.log'}.get(stage,'artifacts/463-pan-'+stage+'-run.log');shutil.copy2(root/outer,dest/(stage+'-outer.log'))
files=git('diff','--name-only',base,source).decode().splitlines();assert files==['crates/hex-ui/src/editor/render.rs','crates/hex-ui/src/editor/resize_pan_tests.rs','crates/hex-ui/src/editor/trim.rs','docs/resize-edge-pan.md','todo.md'];ids={}
for name in files:
 ids[name]={}
 for label,rev in [('before',base),('after',source)]:
  b=git('show',rev+':'+name);q=dest/'source'/label/name;q.parent.mkdir(parents=True,exist_ok=True);q.write_bytes(b);ids[name][label]={'git_blob':git('rev-parse',rev+':'+name).decode().strip(),'sha256':hashlib.sha256(b).hexdigest()}
 assert (root/name).read_bytes()==git('show',source+':'+name)
proof={}
for name in [files[1],files[3],files[4]]:
 assert git('show',source+':'+name).startswith(git('show',base+':'+name));proof[name]='original complete bytes retained as prefix; appended coverage/docs only'
for name in ['AGENTS.md','scripts/local-ci.py','Cargo.toml','Cargo.lock','crates/hex-ui/src/editor/drag_autoscroll.rs','crates/hex-ui/src/editor/controller.rs','crates/hex-ui/src/editor/resize_release_tests.rs','crates/hex-ui/src/editor/resize_terminal_tests.rs','crates/hex-project/src/lib.rs','crates/hex-session/src/lib.rs','crates/hex-engine/src/lib.rs']:
 assert git('show',base+':'+name)==git('show',source+':'+name);proof[name]=hashlib.sha256(git('show',source+':'+name)).hexdigest()
(dest/'source-identities.json').write_text(json.dumps(ids,indent=2)+'\n');(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(dest/'source.patch').write_bytes(git('diff','--binary',base,source));(dest/'source-history.txt').write_bytes(git('log','--format=fuller',base+'..'+source));(dest/'HANDOFF.md').write_text(Path('/tmp/463-handoff.md').read_text()+'\nExact clean receipt:\n'+json.dumps(receipt,indent=2)+'\n')
for name in ['/tmp/463-component.py','/tmp/seal-463.py','/workspace/hex/artifacts/310-approved-guard-runner.py']:
 q=dest/'orchestration'/Path(name).name;q.parent.mkdir(exist_ok=True);shutil.copy2(name,q)
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()};(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n');archive=dest/'hex463-resize-pan-c0cbec36-evidence.tar.gz'
with tarfile.open(archive,'w:gz') as tf:
 for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
 assert len(tf.getmembers())==len(manifest)+1
 for m in tf.getmembers():
  assert m.isfile() and not m.name.startswith('/') and '..' not in Path(m.name).parts
  if m.name in manifest:assert hashlib.sha256(tf.extractfile(m).read()).hexdigest()==manifest[m.name]['sha256']
with tempfile.TemporaryDirectory() as d:
 env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')};subprocess.run(['git','read-tree','--empty'],env=env,check=True);subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True);tree=git('write-tree',env=env).decode().strip();commit=git('commit-tree',tree,'-m','Seal PR463 resize pointer delivery evidence',env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode().strip(),'base':base,'receipt':receipt,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1};(root/'artifacts/463-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:assert git('show',commit+':'+str((dest/name).relative_to(root)))==(dest/name).read_bytes()
print(json.dumps(state,indent=2),flush=True)
