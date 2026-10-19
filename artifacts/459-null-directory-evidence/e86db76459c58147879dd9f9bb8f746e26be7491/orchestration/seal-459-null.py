import ast,hashlib,json,os,shutil,subprocess,tarfile,tempfile,types,sys
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
def git(*a,env=None):return subprocess.check_output(['git',*a],env=env).strip()
def blob(rev,p):return subprocess.check_output(['git','show',rev+':'+p])
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source='e86db76459c58147879dd9f9bb8f746e26be7491';base='34877501eb6209d99458c809c39dcbd40f91bee0';ref='evidence/459-null-directory-e86db764-20261019';dest=root/'artifacts/459-null-directory-evidence'/source
assert git('rev-parse','HEAD').decode()==source and not git('status','--porcelain') and not dest.exists()
p=root/'artifacts/local-ci/459-review-null-directory-final-v2/summary.json';s=json.loads(p.read_text());assert s['exit_code']==0 and s['stale'] is False and s['start_tree']==s['end_tree'] and s['start_tree']['commit']==source and not s['start_tree']['dirty']
for stage in ['red','green','final-v2']:
    shutil.copytree(root/('artifacts/local-ci/459-review-null-directory-'+stage),dest/stage)
    shutil.copytree(root/('artifacts/459-null-directory-'+stage+'-realCargo'),dest/(stage+'-realCargo'))
    shutil.copy2(root/('artifacts/459-null-directory-'+stage+'-run.log'),dest/(stage+'-outer.log'))
shutil.copy2(root/'artifacts/459-null-directory-final-run.log',dest/'startup-error-outer.log')
files=git('diff','--name-only',base,source).decode().splitlines();assert files==['docs/local-ci.md','scripts/local-ci.py','scripts/tests/test_local_ci.py'];ids={}
for name in files:
    a=blob(base,name);b=blob(source,name);assert b==(root/name).read_bytes()
    for label,data in [('before',a),('after',b)]:
        q=dest/'source'/label/name;q.parent.mkdir(parents=True,exist_ok=True);q.write_bytes(data)
    ids[name]={'before_sha256':hashlib.sha256(a).hexdigest(),'after_sha256':hashlib.sha256(b).hexdigest()}
a=blob(base,'scripts/local-ci.py').decode();b=blob(source,'scripts/local-ci.py').decode()
def methods(t):
    c=next(n for n in ast.parse(t).body if isinstance(n,ast.ClassDef) and n.name=='LocalCiRunner')
    return {n.name:ast.get_source_segment(t,n) for n in c.body if isinstance(n,ast.FunctionDef)}
ba,bb=methods(a),methods(b);assert set(ba)==set(bb)
restored=b
for name in ['_load_target_owner','_dependency_content']:restored=restored.replace(bb[name],ba[name])
assert restored==a
proof={'only_two_existing_runner_methods_changed':True,'all_other_method_bytes':{name:hashlib.sha256(text.encode()).hexdigest() for name,text in ba.items() if name not in ['_load_target_owner','_dependency_content']}}
a=blob(base,'scripts/tests/test_local_ci.py');b=blob(source,'scripts/tests/test_local_ci.py');split=a.index(b'\n\nif __name__ == "__main__":');assert b.startswith(a[:split]) and b.endswith(a[split:]);proof['all_original_test_bytes_preserved']=True
for name in ['scripts/launch-restart.py','scripts/launch-restart.swift','launch','scripts/hex-test-build.py','Cargo.toml','Cargo.lock','scripts/check-gpui-patch.py','scripts/check-dependency-patches.py']:
    a=blob(base,name);b=blob(source,name);assert a==b;proof[name]=hashlib.sha256(b).hexdigest()
proof['prior_accepted_evidence']=json.loads((root/'artifacts/459-review-sealed-state.json').read_text())
(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(dest/'source-identities.json').write_text(json.dumps(ids,indent=2)+'\n');(dest/'source.patch').write_bytes(subprocess.check_output(['git','diff','--binary',base,source]));(dest/'source-history.txt').write_bytes(subprocess.check_output(['git','log','--format=fuller',base+'..'+source]))
(dest/'HANDOFF.md').write_text(Path('/tmp/459-null-handoff.md').read_text())
for path in ['/tmp/header-component.py','/tmp/459-review-component.py','/tmp/seal-459-null.py','/tmp/459-null-directory-tests.txt']:
    q=dest/'orchestration';q.mkdir(exist_ok=True);shutil.copy2(path,q/Path(path).name)
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()};(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n');archive=dest/'hex459-null-directory-e86db764-evidence.tar.gz'
with tarfile.open(archive,'w:gz') as tf:
    for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
    assert len(tf.getmembers())==len(manifest)+1
    for m in tf.getmembers():
        assert m.isfile() and not m.name.startswith('/') and '..' not in Path(m.name).parts
        if m.name in manifest:assert hashlib.sha256(tf.extractfile(m).read()).hexdigest()==manifest[m.name]['sha256']
with tempfile.TemporaryDirectory() as d:
    env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')};subprocess.run(['git','read-tree','--empty'],env=env,check=True);subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True);tree=git('write-tree',env=env).decode();commit=subprocess.check_output(['git','commit-tree',tree,'-m','Seal PR459 null owner and directory identity repair evidence'],env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode(),'base':base,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1,'summary_sha256':sha(p),'seconds':s['duration_seconds']};(root/'artifacts/459-null-directory-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:assert subprocess.check_output(['git','show',commit+':'+str((dest/name).relative_to(root))])==(dest/name).read_bytes()
print(json.dumps(state,indent=2),flush=True);subprocess.run(['git','push','origin',ref],check=True)
remote=git('ls-remote','origin','refs/heads/'+ref,'refs/heads/source-fix/459-null-directory-3487').decode();assert commit in remote and source in remote;print('ALL payloads/Git bytes/archive/remote owner+evidence refs verified',flush=True)
