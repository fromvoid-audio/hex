import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
source='7dbf3a16906c83e990d676c2807bb52feb576e3a';base='b017d848dd7df7d7578d59f046ef269e5ea561ba';diagnostic='924ad65b035d0328cabd766d70e773f8f3383507';ref='evidence/423-pair-7dbf3a16-20261019'
dest=root/'artifacts/423-pair-evidence'/source
assert not dest.exists()
def git(*args,env=None):return subprocess.check_output(['git',*args],env=env).strip()
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
assert git('rev-parse','HEAD').decode()==source and not git('status','--porcelain')
p=root/'artifacts/local-ci/header-width-return-423-pair-final/summary.json';s=json.loads(p.read_text())
assert s['stale'] is False and s['exit_code']==0 and s['start_tree']==s['end_tree'] and s['start_tree']['commit']==source and not s['start_tree']['dirty']
for name in ['423-pair-red','423-pair-green','423-pair-final','423-row-b017','423-row-b017-v2','423-row-b017-v3','423-row-main']:
    shutil.copytree(root/('artifacts/local-ci/header-width-return-'+name),dest/name)
shutil.copytree(root/'artifacts/423-pair-return',dest/'raw')
files=git('diff','--name-only',base,source).decode().splitlines();identities={}
assert len(files)==4
for name in files:
    before=subprocess.check_output(['git','show',base+':'+name]);after=subprocess.check_output(['git','show',source+':'+name]);assert (root/name).read_bytes()==after
    for label,data in [('before',before),('after',after)]:
        d=dest/'source'/label/name;d.parent.mkdir(parents=True,exist_ok=True);d.write_bytes(data)
    identities[name]={'before_sha256':hashlib.sha256(before).hexdigest(),'after_sha256':hashlib.sha256(after).hexdigest()}
protected=['crates/hex-ui/src/editor/browser_input.rs','crates/hex-ui/src/editor/browser.rs','crates/hex-widgets/src/numeric_control.rs','crates/hex-widgets/src/numeric_text.rs','crates/hex-widgets/src/tests.rs','crates/hex-widgets/src/lib.rs','crates/hex-widgets/src/style.rs','crates/hex-widgets/src/text_case.rs']
proof={}
for name in protected:
    before=subprocess.check_output(['git','show',base+':'+name]);after=subprocess.check_output(['git','show',source+':'+name]);assert before==after
    proof[name]=hashlib.sha256(after).hexdigest()
name='crates/hex-ui/src/editor/browser_search_selection_ui_test.rs'
a=subprocess.check_output(['git','show',base+':'+name]);b=subprocess.check_output(['git','show',source+':'+name]);assert b.startswith(a)
proof['original_search_selection_tests_unchanged_prefix']=hashlib.sha256(a).hexdigest()
name='crates/hex-widgets/src/text_click_selection.rs'
a=subprocess.check_output(['git','show',base+':'+name]);d=subprocess.check_output(['git','show',diagnostic+':'+name]);assert d.startswith(a)
proof['diagnostic_production_exact_base']=True
proof['row_test_comparison']=json.loads((root/'artifacts/423-pair-return/row-comparison-proof.json').read_text())
(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n')
(dest/'source-identities.json').write_text(json.dumps(identities,indent=2)+'\n')
(dest/'source.patch').write_bytes(subprocess.check_output(['git','diff','--binary',base,source]))
(dest/'diagnostic.patch').write_bytes(subprocess.check_output(['git','diff','--binary',base,diagnostic]))
(dest/'source-history.txt').write_bytes(subprocess.check_output(['git','log','--format=fuller',base+'..'+source]))
for name in ['/tmp/header-component.py','/tmp/seal-423-pair-final.py']:
    d=dest/'orchestration';d.mkdir(exist_ok=True);shutil.copy2(name,d/Path(name).name)
(dest/'HANDOFF.md').write_text(Path('/tmp/423-pair-handoff.md').read_text())
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()}
(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n')
archive=dest/'hex423-pair-7dbf3a16-evidence.tar.gz'
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
    commit=subprocess.check_output(['git','commit-tree',tree,'-m','Seal Hex423 invalidated-pair fix and separate two-baseline row diagnostics'],env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode(),'base':base,'diagnostic':diagnostic,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1,'summary_sha256':sha(p),'seconds':s['duration_seconds']}
(root/'artifacts/423-pair-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:
    data=subprocess.check_output(['git','show',commit+':'+str((dest/name).relative_to(root))]);assert data==(dest/name).read_bytes(),name
print(json.dumps(state,indent=2),flush=True)
subprocess.run(['git','push','origin',ref],check=True)
remote=git('ls-remote','origin','refs/heads/'+ref,'refs/heads/source-fix/423-invalidated-click-pair').decode()
assert commit in remote and source in remote
print('ALL immutable payloads/archive and remote source/evidence refs VERIFIED',flush=True)
