import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
source='429ba3b5502ab747719e9e657498b7b4583f5536';base='6a8dba49fc19158bfeaffb2e72e324a6fa559cf0';diagnostic='dc87538ca9719493d828f0b5d4d259ae9b26d848';ref='evidence/456-length-429ba3b5-20261019'
dest=root/'artifacts/456-length-evidence'/source
assert not dest.exists()
def git(*args,env=None):return subprocess.check_output(['git',*args],env=env).strip()
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
assert git('rev-parse','HEAD').decode()==source and not git('status','--porcelain')
p=root/'artifacts/local-ci/header-width-return-456-length-final/summary.json';s=json.loads(p.read_text())
assert s['stale'] is False and s['exit_code']==0 and s['start_tree']==s['end_tree'] and s['start_tree']['commit']==source and not s['start_tree']['dirty']
for name in ['red','red2','green','final']:
    shutil.copytree(root/('artifacts/local-ci/header-width-return-456-length-'+name),dest/name)
shutil.copytree(root/'artifacts/456-length-return',dest/'raw')
files=git('diff','--name-only',base,source).decode().splitlines();identities={}
assert len(files)==3
for name in files:
    before=subprocess.check_output(['git','show',base+':'+name]);after=subprocess.check_output(['git','show',source+':'+name]);assert (root/name).read_bytes()==after
    for label,data in [('before',before),('after',after)]:
        d=dest/'source'/label/name;d.parent.mkdir(parents=True,exist_ok=True);d.write_bytes(data)
    identities[name]={'before_sha256':hashlib.sha256(before).hexdigest(),'after_sha256':hashlib.sha256(after).hexdigest()}
protected=['crates/hex-project/src/audio_edit/view.rs','crates/hex-project/src/audio_edit/commands.rs','crates/hex-project/tests/audio_view.rs','crates/hex-project/src/device_comparison.rs','crates/hex-storage/src/audio_edit.rs','crates/hex-storage/src/lib.rs','crates/hex-session/src/service.rs','crates/hex-engine/src/audio_transform/mod.rs','crates/hex-ui/src/editor/trim.rs']
proof={}
for name in protected:
    before=subprocess.check_output(['git','show',base+':'+name]);after=subprocess.check_output(['git','show',source+':'+name]);assert before==after
    proof[name]=hashlib.sha256(after).hexdigest()
name='crates/hex-ui/src/editor/arrangement_audio_panel.rs'
a=subprocess.check_output(['git','show',base+':'+name]);b=subprocess.check_output(['git','show',source+':'+name]);d=subprocess.check_output(['git','show',diagnostic+':'+name])
marker=b'#[cfg(test)]\nmod tests'
assert marker in a and marker in b
assert d.split(marker)[0]==a.split(marker)[0]
assert b.split(marker)[1].startswith(a.split(marker)[1][:-2])
proof['diagnostic_production_exact_base']=True
proof['all_original_seven_panel_tests_unchanged_prefix']=hashlib.sha256(a.split(marker)[1][:-2]).hexdigest()
(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n')
(dest/'source-identities.json').write_text(json.dumps(identities,indent=2)+'\n')
(dest/'source.patch').write_bytes(subprocess.check_output(['git','diff','--binary',base,source]))
(dest/'diagnostic.patch').write_bytes(subprocess.check_output(['git','diff','--binary',base,diagnostic]))
(dest/'source-history.txt').write_bytes(subprocess.check_output(['git','log','--format=fuller',base+'..'+source]))
for name in ['/tmp/header-component.py','/tmp/seal-456-length-final.py']:
    d=dest/'orchestration';d.mkdir(exist_ok=True);shutil.copy2(name,d/Path(name).name)
(dest/'HANDOFF.md').write_text(Path('/tmp/456-length-handoff.md').read_text())
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()}
(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n')
archive=dest/'hex456-length-429ba3b5-evidence.tar.gz'
with tarfile.open(archive,'w:gz') as tf:
    for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
    members=tf.getmembers();assert len(members)==len(manifest)+1
    for member in members:
        assert member.isfile() and not member.name.startswith('/') and '..' not in Path(member.name).parts
        data=tf.extractfile(member).read()
        if member.name in manifest:assert hashlib.sha256(data).hexdigest()==manifest[member.name]['sha256']
with tempfile.TemporaryDirectory() as d:
    env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')}
    subprocess.run(['git','read-tree','--empty'],env=env,check=True)
    subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True)
    tree=git('write-tree',env=env).decode()
    commit=subprocess.check_output(['git','commit-tree',tree,'-m','Seal Hex456 actual placed Length controls and real Service metadata preservation'],env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode(),'base':base,'diagnostic':diagnostic,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1,'summary_sha256':sha(p),'seconds':s['duration_seconds']}
(root/'artifacts/456-length-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:
    data=subprocess.check_output(['git','show',commit+':'+str((dest/name).relative_to(root))]);assert data==(dest/name).read_bytes(),name
print(json.dumps(state,indent=2),flush=True)
subprocess.run(['git','push','origin',ref],check=True)
remote=git('ls-remote','origin','refs/heads/'+ref,'refs/heads/source-fix/456-placed-length-controls').decode()
assert commit in remote and source in remote
print('ALL immutable payloads/archive and remote source/evidence refs VERIFIED',flush=True)
