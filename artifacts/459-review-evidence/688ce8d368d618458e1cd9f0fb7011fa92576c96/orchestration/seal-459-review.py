import ast,hashlib,json,os,shutil,subprocess,tarfile,tempfile,types,sys
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
def git(*a,env=None):return subprocess.check_output(['git',*a],env=env).strip()
def blob(rev,p):return subprocess.check_output(['git','show',rev+':'+p])
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source='688ce8d368d618458e1cd9f0fb7011fa92576c96';base='c6464224fc7ff6b61c942868739828f1fbace5f6';ref='evidence/459-review-688ce8d3-20261019';dest=root/'artifacts/459-review-evidence'/source
assert git('rev-parse','HEAD').decode()==source and not git('status','--porcelain') and not dest.exists()
p=root/'artifacts/local-ci/459-review-final/summary.json';s=json.loads(p.read_text());assert s['exit_code']==0 and s['stale'] is False and s['start_tree']==s['end_tree'] and s['start_tree']['commit']==source and not s['start_tree']['dirty']
for name in ['459-review-red','459-review-symlink-red','459-review-green']:
    shutil.copytree(root/('artifacts/local-ci/header-width-return-'+name),dest/name)
shutil.copytree(root/'artifacts/local-ci/459-review-final',dest/'final')
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
for name in ['_dependency_content','run']:restored=restored.replace(bb[name],ba[name])
assert restored==a
proof={'only_two_existing_runner_methods_changed':True,'all_other_method_bytes':{name:hashlib.sha256(text.encode()).hexdigest() for name,text in ba.items() if name not in ['_dependency_content','run']}}
a=blob(base,'scripts/tests/test_local_ci.py');b=blob(source,'scripts/tests/test_local_ci.py');split=a.index(b'\n\nif __name__ == "__main__":');assert b.startswith(a[:split]) and b.endswith(a[split:]);proof['all_original_test_bytes_preserved']=True
for name in ['scripts/launch-restart.py','scripts/launch-restart.swift','launch','scripts/hex-test-build.py','Cargo.toml','Cargo.lock','scripts/check-gpui-patch.py','scripts/check-dependency-patches.py']:
    a=blob(base,name);b=blob(source,name);assert a==b;proof[name]=hashlib.sha256(b).hexdigest()
# Read-only, private-input comparison proves unchanged digest encoding on stable trees.
old=types.ModuleType('old_local_ci459');old.__file__=str(root/'scripts/local-ci.py');sys.modules[old.__name__]=old;exec(compile(blob(base,'scripts/local-ci.py'),old.__file__,'exec'),old.__dict__)
new=types.ModuleType('new_local_ci459');new.__file__=old.__file__;sys.modules[new.__name__]=new;exec(compile(blob(source,'scripts/local-ci.py'),new.__file__,'exec'),new.__dict__)
equivalence=[]
with tempfile.TemporaryDirectory() as directory:
    package=Path(directory)/'package';package.mkdir();(package/'Cargo.toml').write_text('[package]\nname="fixture"\nversion="0.1.0"\n');(package/'src').mkdir();(package/'src/lib.rs').write_bytes(b'pub const VALUE: u32 = 7;\n');target=package/'target';target.mkdir();(target/'generated').write_bytes(b'excluded');(package/'.git').mkdir();(package/'.git/config').write_bytes(b'excluded')
    for label in ['ordinary','file-symlink','executable']:
        if label=='file-symlink':(package/'alias.rs').symlink_to(package/'src/lib.rs')
        if label=='executable':(package/'src/lib.rs').chmod(0o755)
        left=old.LocalCiRunner._dependency_content(package,target);right=new.LocalCiRunner._dependency_content(package,target);assert left==right;equivalence.append({'case':label,**right})
proof['old_new_stable_input_digest_equivalence']=equivalence
proof['prior_accepted_evidence']=json.loads((root/'artifacts/459-sealed-state.json').read_text())
(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(dest/'source-identities.json').write_text(json.dumps(ids,indent=2)+'\n');(dest/'source.patch').write_bytes(subprocess.check_output(['git','diff','--binary',base,source]));(dest/'source-history.txt').write_bytes(subprocess.check_output(['git','log','--format=fuller',base+'..'+source]))
(dest/'HANDOFF.md').write_text(Path('/tmp/459-review-handoff.md').read_text())
for path in ['/tmp/header-component.py','/tmp/459-review-component.py','/tmp/seal-459-review.py']:
    q=dest/'orchestration';q.mkdir(exist_ok=True);shutil.copy2(path,q/Path(path).name)
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()};(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n');archive=dest/'hex459-review-688ce8d3-evidence.tar.gz'
with tarfile.open(archive,'w:gz') as tf:
    for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
    assert len(tf.getmembers())==len(manifest)+1
    for m in tf.getmembers():
        assert m.isfile() and not m.name.startswith('/') and '..' not in Path(m.name).parts
        if m.name in manifest:assert hashlib.sha256(tf.extractfile(m).read()).hexdigest()==manifest[m.name]['sha256']
with tempfile.TemporaryDirectory() as d:
    env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')};subprocess.run(['git','read-tree','--empty'],env=env,check=True);subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True);tree=git('write-tree',env=env).decode();commit=subprocess.check_output(['git','commit-tree',tree,'-m','Seal PR459 three review repairs and exact private-runner evidence'],env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode(),'base':base,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1,'summary_sha256':sha(p),'seconds':s['duration_seconds']};(root/'artifacts/459-review-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:assert subprocess.check_output(['git','show',commit+':'+str((dest/name).relative_to(root))])==(dest/name).read_bytes()
print(json.dumps(state,indent=2),flush=True);subprocess.run(['git','push','origin',ref],check=True)
remote=git('ls-remote','origin','refs/heads/'+ref,'refs/heads/source-fix/459-review-c646').decode();assert commit in remote and source in remote;print('ALL payloads/Git bytes/archive/remote owner+evidence refs verified',flush=True)
