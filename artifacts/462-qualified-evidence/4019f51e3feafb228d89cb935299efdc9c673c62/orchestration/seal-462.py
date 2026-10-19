import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
def git(*a,env=None):return subprocess.check_output(['git',*a],env=env)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source=git('rev-parse','HEAD').decode().strip();base='e9d216eb5ae60bf2917e03bdaff8224aeb3b7767';dest=root/'artifacts/462-qualified-evidence'/source;ref='evidence/462-clip-local-'+source[:8]+'-20261019'
assert not git('status','--porcelain') and not dest.exists()
p=root/'artifacts/local-ci/462-resize-paint-final/summary.json';s=json.loads(p.read_text());assert s['exit_code']==0 and not s['stale'] and s['start_tree']==s['end_tree'] and not s['start_tree']['dirty'] and s['start_tree']['commit']==source
receipt={'source':source,'seconds':s['duration_seconds'],'summary_sha256':sha(p),'content_sha256':s['start_tree']['content_sha256']};dest.mkdir(parents=True)
for stage in ['red','green','green-v2','green-v3','baseline-cancel-diagnostic','baseline-scene-probe','scoped-green','final']:
 shutil.copytree(root/('artifacts/local-ci/462-resize-paint-'+stage),dest/'runs'/stage);shutil.copy2(root/('artifacts/462-paint-'+stage+'-run.log'),dest/(stage+'-outer.log'))
shutil.copytree(root/'artifacts/462-paint-diagnostics',dest/'preserved-2871-diagnostics')
files=git('diff','--name-only',base,source).decode().splitlines();assert files==['crates/hex-ui/src/editor/resize_release_tests.rs','crates/hex-ui/src/editor/tracks.rs','docs/resize-edge-pan.md','todo.md'];ids={}
for name in files:
 ids[name]={}
 for label,rev in [('before',base),('after',source)]:
  b=git('show',rev+':'+name);q=dest/'source'/label/name;q.parent.mkdir(parents=True,exist_ok=True);q.write_bytes(b);ids[name][label]={'git_blob':git('rev-parse',rev+':'+name).decode().strip(),'sha256':hashlib.sha256(b).hexdigest()}
 assert (root/name).read_bytes()==git('show',source+':'+name)
callback='''            .drag_over::<TrimDrag>(move |style, drag, _, _| {
                if drag.source_track == track_id {
                    style
                        .border_1()
                        .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
                } else {
                    style
                }
            })
'''
assert git('show',base+':'+files[1]).decode().replace(callback,'',1)==git('show',source+':'+files[1]).decode()
proof={'production_exact_nine_line_callback_removal':True,'2871_original_evidence_preserved':'14f27991c613d352f3696d585ffac7624666b6b7','2871_original_checkpoint_preserved':'717e2829c9b47bd8aa7eda86dd791f48dc2e4210','not_claiming_inherited_preview_disappearance_fixed':True}
for name in [files[0],files[2],files[3]]:
 assert git('show',source+':'+name).startswith(git('show',base+':'+name));proof[name]='all original bytes exact prefix'
for name in ['AGENTS.md','scripts/local-ci.py','Cargo.toml','Cargo.lock','crates/hex-ui/src/editor/render.rs','crates/hex-ui/src/editor/trim.rs','crates/hex-ui/src/editor/drag_autoscroll.rs','crates/hex-ui/src/editor/controller.rs','crates/hex-ui/src/editor/clip_border_ui_test.rs','crates/hex-project/src/edit.rs','crates/hex-project/src/midi.rs','crates/hex-session/src/lib.rs','crates/hex-engine/src/lib.rs']:
 b=git('show',base+':'+name);assert b==git('show',source+':'+name);proof[name]=hashlib.sha256(b).hexdigest()
(dest/'source-identities.json').write_text(json.dumps(ids,indent=2)+'\n');(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(dest/'source.patch').write_bytes(git('diff','--binary',base,source));(dest/'source-history.txt').write_bytes(git('log','--format=fuller',base+'..'+source));(dest/'HANDOFF.md').write_text(Path('/tmp/462-handoff.md').read_text()+'\nClean exact receipt:\n'+json.dumps(receipt,indent=2)+'\n')
for name in ['/tmp/462-component.py','/tmp/seal-462.py','/workspace/hex/artifacts/310-approved-guard-runner.py']:
 q=dest/'orchestration'/Path(name).name;q.parent.mkdir(exist_ok=True);shutil.copy2(name,q)
manifest={p.relative_to(dest).as_posix():{'size':p.stat().st_size,'sha256':sha(p)} for p in sorted(dest.rglob('*')) if p.is_file()};(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n');archive=dest/('hex462-clip-local-'+source[:8]+'-evidence.tar.gz')
with tarfile.open(archive,'w:gz') as tf:
 for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
 assert len(tf.getmembers())==len(manifest)+1
 for m in tf.getmembers():
  assert m.isfile() and not m.name.startswith('/') and '..' not in Path(m.name).parts
  if m.name in manifest:assert hashlib.sha256(tf.extractfile(m).read()).hexdigest()==manifest[m.name]['sha256']
with tempfile.TemporaryDirectory() as d:
 env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')};subprocess.run(['git','read-tree','--empty'],env=env,check=True);subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True);tree=git('write-tree',env=env).decode().strip();commit=git('commit-tree',tree,'-m','Seal qualified PR462 clip-local resize styling source and scoped evidence',env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode().strip(),'base':base,'receipt':receipt,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1};(root/'artifacts/462-qualified-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:assert git('show',commit+':'+str((dest/name).relative_to(root)))==(dest/name).read_bytes()
print(json.dumps(state,indent=2),flush=True)
