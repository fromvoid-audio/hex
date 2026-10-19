import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
def git(*a,env=None):return subprocess.check_output(['git',*a],env=env).strip()
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source=git('rev-parse','HEAD').decode();base='03db8f925343a70192eb1983184bea2eed7a2ad2';main='2683942bcb355010ea3f397d3428213178ea8ad9';ref='evidence/460-arrows-'+source[:8]+'-20261019'
dest=root/'artifacts/460-arrows-evidence'/source
assert not dest.exists() and not git('status','--porcelain')
p=root/'artifacts/local-ci/header-width-return-460-arrows-qualified/summary.json';s=json.loads(p.read_text())
assert s['stale'] is False and s['exit_code']==0 and s['start_tree']==s['end_tree'] and s['start_tree']['commit']==source and not s['start_tree']['dirty']
for name in ['460-arrows-first','460-arrows-final','460-arrows-qualified','423-row-main','423-row-b017-v3']:
    shutil.copytree(root/('artifacts/local-ci/header-width-return-'+name),dest/name)
files=git('diff','--name-only',base,source).decode().splitlines();assert len(files)==5
identities={}
for name in files:
    a=subprocess.check_output(['git','show',base+':'+name]);b=subprocess.check_output(['git','show',source+':'+name]);assert b==(root/name).read_bytes()
    for label,data in [('before',a),('after',b)]:
        d=dest/'source'/label/name;d.parent.mkdir(parents=True,exist_ok=True);d.write_bytes(data)
    identities[name]={'before_sha256':hashlib.sha256(a).hexdigest(),'after_sha256':hashlib.sha256(b).hexdigest()}
protected=['crates/hex-ui/src/editor/browser_input.rs','crates/hex-ui/src/editor/browser/audition.rs','crates/hex-ui/src/editor/browser/layout.rs','crates/hex-ui/src/editor/render.rs','crates/hex-ui/src/editor/controller.rs','crates/hex-widgets/src/numeric_control.rs','crates/hex-widgets/src/numeric_text.rs','crates/hex-widgets/src/lib.rs','crates/hex-widgets/src/style.rs','crates/hex-widgets/src/text_case.rs','crates/hex-project/src/lib.rs']
proof={}
for name in protected:
    a=subprocess.check_output(['git','show',base+':'+name]);b=subprocess.check_output(['git','show',source+':'+name]);assert a==b;proof[name]=hashlib.sha256(b).hexdigest()
a=subprocess.check_output(['git','show',base+':crates/hex-ui/src/editor/browser_ui_test.rs']);b=subprocess.check_output(['git','show',source+':crates/hex-ui/src/editor/browser_ui_test.rs']);assert b.startswith(a);proof['original_browser_ui_tests_unchanged_prefix']=hashlib.sha256(a).hexdigest()
assert git('rev-parse',base+'^{tree}')==git('rev-parse',main+'^{tree}')
proof['plan_tree_equals_diagnostic_main']=git('rev-parse',main+'^{tree}').decode()
proof['baseline_evidence']='06bf54aba08f081a808511adb0cfdcb03e2d2483; same five actual RED and selected control PASS on b017 and main2683, independently verified by Root'
for name in ['row-comparison-proof.json','row-main.patch','row-b017.patch']:
    shutil.copy2(root/'artifacts/423-pair-return'/name,dest/name)
(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(dest/'source-identities.json').write_text(json.dumps(identities,indent=2)+'\n')
(dest/'source.patch').write_bytes(subprocess.check_output(['git','diff','--binary',base,source]));(dest/'source-history.txt').write_bytes(subprocess.check_output(['git','log','--format=fuller',base+'..'+source]))
for path in ['/tmp/header-component.py','/tmp/seal-460-arrows.py']:
    d=dest/'orchestration';d.mkdir(exist_ok=True);shutil.copy2(path,d/Path(path).name)
(dest/'HANDOFF.md').write_text(Path('/tmp/460-arrows-handoff.md').read_text())
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()}
(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n');archive=dest/('hex460-arrows-'+source[:8]+'-evidence.tar.gz')
with tarfile.open(archive,'w:gz') as tf:
    for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
    assert len(tf.getmembers())==len(manifest)+1
    for m in tf.getmembers():
        assert m.isfile() and not m.name.startswith('/') and '..' not in Path(m.name).parts
        if m.name in manifest:assert hashlib.sha256(tf.extractfile(m).read()).hexdigest()==manifest[m.name]['sha256']
with tempfile.TemporaryDirectory() as d:
    env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')};subprocess.run(['git','read-tree','--empty'],env=env,check=True);subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True)
    tree=git('write-tree',env=env).decode();commit=subprocess.check_output(['git','commit-tree',tree,'-m','Seal PR460 arrow eligibility source and original negative histories'],env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode(),'base':base,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1,'summary_sha256':sha(p),'seconds':s['duration_seconds']}
(root/'artifacts/460-arrows-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:assert subprocess.check_output(['git','show',commit+':'+str((dest/name).relative_to(root))])==(dest/name).read_bytes()
print(json.dumps(state,indent=2),flush=True)
subprocess.run(['git','push','origin',ref],check=True)
remote=git('ls-remote','origin','refs/heads/'+ref,'refs/heads/pulse/task-2858-g1-4ba7977c9758').decode();assert commit in remote and source in remote
print('ALL payloads/Git bytes/archive/remote refs verified',flush=True)
