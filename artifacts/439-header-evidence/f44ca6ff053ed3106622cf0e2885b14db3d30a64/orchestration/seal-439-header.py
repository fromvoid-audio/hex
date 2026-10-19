import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
def git(*a,env=None):return subprocess.check_output(['git',*a],env=env).strip()
def blob(r,p):return subprocess.check_output(['git','show',r+':'+p])
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source='f44ca6ff053ed3106622cf0e2885b14db3d30a64';base='fb4cb8be3896b73c8ebda76027bbcbfbd02622ec';prior='ce38ef26fb40382cd7c4629ca4cda7a44b7a339d';main='9d1d0f0bbd14fcbc2ff0eecb975f52db65a12aa2';ref='evidence/439-header-'+source[:8]+'-20261019'
dest=root/'artifacts/439-header-evidence'/source
assert git('rev-parse','HEAD').decode()==source and not git('status','--porcelain') and not dest.exists()
p=root/'artifacts/local-ci/header-width-return-439-qualified-f44ca6ff/summary.json';s=json.loads(p.read_text());assert s['stale'] is False and s['exit_code']==0 and s['start_tree']==s['end_tree'] and s['start_tree']['commit']==source and not s['start_tree']['dirty'] and len(s['steps'])==13 and all(x['status']=='passed' for x in s['steps'])
for name in ['439-baseline','439-filled-v1','439-filled-v2','439-first-functional','439-functional-v2','439-functional-v3','439-layout-v1','439-functional-v4','439-functional-v5','439-functional-v6','439-final-6953c416','439-qualified-f44ca6ff']:
 shutil.copytree(root/('artifacts/local-ci/header-width-return-'+name),dest/name)
shutil.copytree(root/'artifacts/439-reference-comparison',dest/'visual');shutil.copytree(root/'artifacts/439-baseline-recovered',dest/'baseline-source-recovered')
files=git('diff','--name-only',base,source).decode().splitlines();assert len(files)==16;ids={}
for name in files:
 a=blob(base,name);b=blob(source,name);assert b==(root/name).read_bytes()
 for label,data in [('before',a),('after',b)]:
  q=dest/'source'/label/name;q.parent.mkdir(parents=True,exist_ok=True);q.write_bytes(data)
 ids[name]={'before_git_blob':git('rev-parse',base+':'+name).decode(),'after_git_blob':git('rev-parse',source+':'+name).decode(),'before_sha256':hashlib.sha256(a).hexdigest(),'after_sha256':hashlib.sha256(b).hexdigest()}
proof={}
for name in ['crates/hex-project/src/lib.rs','crates/hex-project/src/edit.rs','crates/hex-project/src/returns.rs','crates/hex-engine/src/lib.rs','crates/hex-engine/src/compile.rs','crates/hex-audio/src/lib.rs','crates/hex-session/src/service.rs','crates/hex-ui/src/editor/mod.rs','crates/hex-ui/src/editor/render.rs','crates/hex-ui/src/editor/controller.rs','crates/hex-ui/src/editor/track_layout.rs','crates/hex-widgets/src/numeric_text.rs','crates/hex-widgets/src/text_case.rs','crates/hex-widgets/src/style.rs','scripts/local-ci.py']:
 a=blob(base,name);b=blob(source,name);assert a==b;proof[name]=hashlib.sha256(b).hexdigest()
a=blob(base,'scripts/check-ui-style.py').decode();b=blob(source,'scripts/check-ui-style.py').decode();start=b.index('        # Header mixer composition uses typed controls;');end=b.index('            line = source.count',start);restored=b[:start]+'        if not suffix.startswith(widgets) and not typed_error and not fixture_canvas:\n'+b[end:];assert restored==a;proof['checker_only_file_local_component_registration']=True
for rev in [base,prior,main,'3b78d458e05b4aeb3d604b7e3c86e424a84154c3']:
 subprocess.run(['git','merge-base','--is-ancestor',rev,source],check=True)
proof['preserved_ancestors']=[base,prior,main];proof['integrated_before_tree_equals_actual_main']=git('rev-parse',base+'^{tree}')==git('rev-parse',main+'^{tree}');assert proof['integrated_before_tree_equals_actual_main']
(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(dest/'source-identities.json').write_text(json.dumps(ids,indent=2)+'\n')
(dest/'source.patch').write_bytes(subprocess.check_output(['git','diff','--binary',base,source]));(dest/'source-history.txt').write_bytes(subprocess.check_output(['git','log','--first-parent','--format=fuller',prior+'..'+source]));(dest/'HANDOFF.md').write_text(Path('/tmp/439-handoff.md').read_text())
for name in ['header-component.py','439-session-patch.py','439-ui-mix-patch.py','439-header-patch.py','439-final-functional.py','439-style-registration.py','439-render-evidence.py','439-recover-baseline.py','seal-439-header.py']:
 q=dest/'orchestration';q.mkdir(exist_ok=True);shutil.copy2(Path('/tmp')/name,q/name)
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()};(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n');archive=dest/('hex439-header-'+source[:8]+'-evidence.tar.gz')
with tarfile.open(archive,'w:gz') as tf:
 for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
 assert len(tf.getmembers())==len(manifest)+1
 for m in tf.getmembers():
  assert m.isfile() and not m.name.startswith('/') and '..' not in Path(m.name).parts
  if m.name in manifest:assert hashlib.sha256(tf.extractfile(m).read()).hexdigest()==manifest[m.name]['sha256']
with tempfile.TemporaryDirectory() as d:
 env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')};subprocess.run(['git','read-tree','--empty'],env=env,check=True);subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True);tree=git('write-tree',env=env).decode();commit=subprocess.check_output(['git','commit-tree',tree,'-m','Seal PR439 supported mixer component, original RED and exact source checks'],env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode(),'base':base,'main':main,'prior':prior,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1,'summary_sha256':sha(p),'seconds':s['duration_seconds'],'source_content_sha256':s['source_identity']['content_sha256']};(root/'artifacts/439-header-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:assert subprocess.check_output(['git','show',commit+':'+str((dest/name).relative_to(root))])==(dest/name).read_bytes()
print(json.dumps(state,indent=2),flush=True);subprocess.run(['git','push','origin',ref],check=True)
remote=git('ls-remote','origin','refs/heads/'+ref,'refs/heads/pulse/task-2835-g1-decc6ba9bc1a').decode();assert commit in remote and source in remote;print('ALL payloads, Git bytes, archive and remote refs verified',flush=True)
