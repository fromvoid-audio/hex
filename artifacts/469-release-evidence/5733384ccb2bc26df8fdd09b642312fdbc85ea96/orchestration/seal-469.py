import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
def git(*args,env=None):return subprocess.check_output(['git',*args],env=env)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source=git('rev-parse','HEAD').decode().strip();base='b9e0e3afdcf05dd8c6c4a9331d1b822606151c0d';ref='evidence/469-captured-release-'+source[:8]+'-20261019';dest=root/'artifacts/469-release-evidence'/source
assert not git('status','--porcelain') and not dest.exists()
p=root/'artifacts/local-ci/469-outside-release-final/summary.json';s=json.loads(p.read_text());assert s['exit_code']==0 and not s['stale'] and s['start_tree']==s['end_tree'] and not s['start_tree']['dirty'] and s['start_tree']['commit']==source
receipt={'source':source,'seconds':s['duration_seconds'],'summary_sha256':sha(p),'content_sha256':s['start_tree']['content_sha256']}
dest.mkdir(parents=True)
for stage in ['red','red-v2','green','outside-corrected-red','controls','final']:
 shutil.copytree(root/('artifacts/local-ci/469-outside-release-'+stage),dest/'runs'/stage);shutil.copy2(root/('artifacts/469-release-'+stage+'-run.log'),dest/(stage+'-outer.log'))
files=git('diff','--name-only',base,source).decode().splitlines();assert files==['crates/hex-ui/src/editor/resize_pan_tests.rs','crates/hex-ui/src/editor/resize_release_tests.rs','crates/hex-ui/src/editor/trim.rs','docs/resize-edge-pan.md','todo.md'];ids={}
for name in files:
 ids[name]={}
 for label,rev in [('before',base),('after',source)]:
  b=git('show',rev+':'+name);q=dest/'source'/label/name;q.parent.mkdir(parents=True,exist_ok=True);q.write_bytes(b);ids[name][label]={'git_blob':git('rev-parse',rev+':'+name).decode().strip(),'sha256':hashlib.sha256(b).hexdigest()}
 assert (root/name).read_bytes()==git('show',source+':'+name)
assert git('diff',base,source,'--','crates/hex-ui/src/editor/trim.rs')==Path('/tmp/469-release-fix.patch').read_bytes()
proof={'production_patch_exactly_declared_existing_finish_helper':True,'source_base_tree_equals_frozen463':git('rev-parse',base+'^{tree}').decode().strip()=='07236aae104738faa7a41d1cc257f2828406ce52','old_cancellation_tests_replaced_only_for_new_user_requirement':'captured othertrack/genericoutside now require validcommit; raw original beforeblobs retained'}
for name in ['AGENTS.md','scripts/local-ci.py','Cargo.toml','Cargo.lock','crates/hex-ui/src/editor/render.rs','crates/hex-ui/src/editor/drag_autoscroll.rs','crates/hex-ui/src/editor/controller.rs','crates/hex-ui/src/editor/resize_terminal_tests.rs','crates/hex-project/src/lib.rs','crates/hex-session/src/lib.rs','crates/hex-engine/src/lib.rs']:
 assert git('show',base+':'+name)==git('show',source+':'+name);proof[name]=hashlib.sha256(git('show',source+':'+name)).hexdigest()
(dest/'source-identities.json').write_text(json.dumps(ids,indent=2)+'\n');(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(dest/'source.patch').write_bytes(git('diff','--binary',base,source));(dest/'source-history.txt').write_bytes(git('log','--format=fuller',base+'..'+source));(dest/'HANDOFF.md').write_text(Path('/tmp/469-handoff.md').read_text()+'\nExact clean receipt:\n'+json.dumps(receipt,indent=2)+'\n')
for name in ['/tmp/469-component.py','/tmp/seal-469.py','/tmp/469-release-fix.patch','/tmp/469-tests.py','/tmp/469-expand.py','/tmp/469-pan-chain.py','/workspace/hex/artifacts/310-approved-guard-runner.py']:
 q=dest/'orchestration'/Path(name).name;q.parent.mkdir(exist_ok=True);shutil.copy2(name,q)
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()};(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n');archive=dest/('hex469-captured-release-'+source[:8]+'-evidence.tar.gz')
with tarfile.open(archive,'w:gz') as tf:
 for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
 assert len(tf.getmembers())==len(manifest)+1
 for m in tf.getmembers():
  assert m.isfile() and not m.name.startswith('/') and '..' not in Path(m.name).parts
  if m.name in manifest:assert hashlib.sha256(tf.extractfile(m).read()).hexdigest()==manifest[m.name]['sha256']
with tempfile.TemporaryDirectory() as d:
 env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')};subprocess.run(['git','read-tree','--empty'],env=env,check=True);subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True);tree=git('write-tree',env=env).decode().strip();commit=git('commit-tree',tree,'-m','Seal PR469 captured resize release evidence',env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode().strip(),'base':base,'dependency':'463/c0cbec36990546ceefbba22656d3819ddd143e5a','receipt':receipt,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1};(root/'artifacts/469-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:assert git('show',commit+':'+str((dest/name).relative_to(root)))==(dest/name).read_bytes()
print(json.dumps(state,indent=2),flush=True)
