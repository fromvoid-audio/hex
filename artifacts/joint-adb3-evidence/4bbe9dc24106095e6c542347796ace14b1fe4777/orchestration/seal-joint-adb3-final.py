import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
source='4bbe9dc24106095e6c542347796ace14b1fe4777';base='adb3ca615efabe837477dad8f07de0aa32d18020';diagnostic='49469e7c5791f2f5fb548e1205cc16f0c31910f7';ref='evidence/419-458-adb3-4bbe9dc2-20261019'
dest=root/'artifacts/joint-adb3-evidence'/source
assert not dest.exists()
def git(*args,env=None):return subprocess.check_output(['git',*args],env=env).strip()
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
assert git('rev-parse','HEAD').decode()==source and not git('status','--porcelain')
p=root/'artifacts/local-ci/header-width-return-joint-adb3-final/summary.json';s=json.loads(p.read_text())
assert s['stale'] is False and s['exit_code']==0 and s['start_tree']==s['end_tree'] and s['start_tree']['commit']==source and not s['start_tree']['dirty']
for name in ['red','red2','red3','red4','green','final']:
    shutil.copytree(root/('artifacts/local-ci/header-width-return-joint-adb3-'+name),dest/name)
shutil.copytree(root/'artifacts/joint-adb3-return',dest/'raw')
files=git('diff','--name-only',base,source).decode().splitlines();identities={}
assert len(files)==6
for name in files:
    before=subprocess.check_output(['git','show',base+':'+name]);after=subprocess.check_output(['git','show',source+':'+name]);assert (root/name).read_bytes()==after
    for label,data in [('before',before),('after',after)]:
        d=dest/'source'/label/name;d.parent.mkdir(parents=True,exist_ok=True);d.write_bytes(data)
    identities[name]={'before_sha256':hashlib.sha256(before).hexdigest(),'after_sha256':hashlib.sha256(after).hexdigest()}
protected=['crates/hex-ui/src/editor/workspace_view_ui_test.rs','crates/hex-ui/src/editor/device_panel/tests/operator_tab_tests.rs','crates/hex-ui/src/editor/gain_lane/tests.rs','crates/hex-ui/src/editor/gain_lane/paint.rs','crates/hex-ui/src/editor/controller.rs','crates/hex-ui/src/editor/render.rs','crates/hex-ui/src/editor/tracks.rs']
proof={}
for name in protected:
    before=subprocess.check_output(['git','show',base+':'+name]);after=subprocess.check_output(['git','show',source+':'+name]);assert before==after
    proof[name]=hashlib.sha256(after).hexdigest()
nav='crates/hex-ui/src/editor/device_panel/tests/navigation_ui_tests.rs'
a=subprocess.check_output(['git','show',base+':'+nav]);b=subprocess.check_output(['git','show',source+':'+nav]);assert b.startswith(a)
proof['original_navigation_test_body_unchanged_prefix']=hashlib.sha256(a).hexdigest()
for name in ['crates/hex-ui/src/editor/workspace_view.rs','crates/hex-ui/src/editor/device_panel.rs','crates/hex-ui/src/editor/gain_lane/mod.rs']:
    assert subprocess.check_output(['git','show',base+':'+name])==subprocess.check_output(['git','show',diagnostic+':'+name])
proof['executed_red_diagnostic_production_equals_base']=True
(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n')
(dest/'source-identities.json').write_text(json.dumps(identities,indent=2)+'\n')
(dest/'source.patch').write_bytes(subprocess.check_output(['git','diff','--binary',base,source]))
(dest/'diagnostic.patch').write_bytes(subprocess.check_output(['git','diff','--binary',base,diagnostic]))
(dest/'source-history.txt').write_bytes(subprocess.check_output(['git','log','--format=fuller',base+'..'+source]))
for name in ['/tmp/header-component.py','/tmp/seal-joint-adb3-final.py']:
    d=dest/'orchestration';d.mkdir(exist_ok=True);shutil.copy2(name,d/Path(name).name)
(dest/'HANDOFF.md').write_text(Path('/tmp/joint-adb3-handoff.md').read_text())
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()}
(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n')
archive=dest/'hex419-458-adb3-4bbe9dc2-evidence.tar.gz'
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
    commit=subprocess.check_output(['git','commit-tree',tree,'-m','Seal joint419/458 last-owner menu and gain pointer actual-input repair'],env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode(),'base':base,'diagnostic':diagnostic,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1,'summary_sha256':sha(p),'seconds':s['duration_seconds']}
(root/'artifacts/joint-adb3-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:
    data=subprocess.check_output(['git','show',commit+':'+str((dest/name).relative_to(root))]);assert data==(dest/name).read_bytes(),name
print(json.dumps(state,indent=2),flush=True)
subprocess.run(['git','push','origin',ref],check=True)
remote=git('ls-remote','origin','refs/heads/'+ref,'refs/heads/source-fix/419-adb3-menu-gain-capture').decode()
assert commit in remote and source in remote
print('ALL immutable payloads/archive and remote source/evidence refs VERIFIED',flush=True)
