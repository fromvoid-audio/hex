import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
def git(*a,env=None):return subprocess.check_output(['git',*a],env=env).strip()
def blob(rev,path):
    r=subprocess.run(['git','show',rev+':'+path],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
    return r.stdout if r.returncode==0 else None
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source=git('rev-parse','HEAD').decode();base='2683942bcb355010ea3f397d3428213178ea8ad9';prior='c525d0400a862d6e686948bf20410337db0001bb';integrated='a22e4ec11e6e5ed9143b91b77faa24d98e6e3ef5';ref='evidence/329-loop-'+source[:8]+'-20261019'
dest=root/'artifacts/329-loop-evidence'/source
assert not dest.exists() and not git('status','--porcelain')
p=root/'artifacts/local-ci/header-width-return-329-loop-qualified/summary.json';s=json.loads(p.read_text())
assert s['stale'] is False and s['exit_code']==0 and s['start_tree']==s['end_tree'] and s['start_tree']['commit']==source and not s['start_tree']['dirty'] and len(s['steps'])==11
for name in ['329-marker-red','329-loop-functional','329-loop-functional-v2','329-loop-functional-v3','329-loop-final','329-loop-qualified']:
    shutil.copytree(root/('artifacts/local-ci/header-width-return-'+name),dest/name)
shutil.copytree(root/'artifacts/329-loop-return',dest/'original-records')
files=git('diff','--name-only',base,source).decode().splitlines();assert len(files)==6
identities={}
for name in files:
    a=blob(base,name);b=blob(source,name);assert b is not None and b==(root/name).read_bytes()
    for label,data in [('before',a),('after',b)]:
        if data is not None:
            d=dest/'source'/label/name;d.parent.mkdir(parents=True,exist_ok=True);d.write_bytes(data)
    identities[name]={'before_exists':a is not None,'before_sha256':hashlib.sha256(a).hexdigest() if a is not None else None,'after_sha256':hashlib.sha256(b).hexdigest()}
proof={}
for name in ['crates/hex-ui/src/editor/mod.rs','crates/hex-ui/src/editor/loop_region.rs','crates/hex-ui/src/editor/transport.rs','crates/hex-ui/src/editor/controller.rs','crates/hex-ui/src/editor/musical_time.rs','crates/hex-project/src/lib.rs','crates/hex-project/src/timebase/tempo.rs','crates/hex-session/src/service.rs']:
    a=blob(base,name);b=blob(source,name);assert a==b;proof[name]=hashlib.sha256(b).hexdigest()
name='crates/hex-ui/src/editor/render.rs';a=blob(base,name).decode();b=blob(source,name).decode()
a=a.replace('                    view.tuning.shift = event.modifiers.shift;','                    view.tuning.shift = event.modifiers.shift;\n                    if view.refresh_loop_rail_modifiers(event.modifiers, window) {\n                        cx.notify();\n                    }').replace('                if view.cancel_project_spectrum_resize(cx)\n                    | view.cancel_trim_gesture(cx)','                if view.cancel_loop_rail_gesture()\n                    | view.cancel_project_spectrum_resize(cx)\n                    | view.cancel_trim_gesture(cx)')
assert a==b;proof['render_only_two_declared_callback_hunks']=True
name='crates/hex-ui/src/editor/loop_rail_ui_test.rs';a=blob(base,name).decode();b=blob(source,name).decode();marker='#[gpui::test]\nfn loop_rail_paint_previews'
a=a[a.index(marker):];b=b[b.index(marker):]
a=a.replace('fn loop_rail_click_activates_resizable_ends_preserve_enable_and_toolbar_disables(', 'fn loop_rail_command_click_activates_resizable_ends_preserve_enable_and_toolbar_disables(')
a=a.replace('    visual.simulate_mouse_down(midpoint, MouseButton::Left, Modifiers::default());\n    visual.simulate_mouse_up(midpoint, MouseButton::Left, Modifiers::default());','    let command = Modifiers {\n        platform: true,\n        ..Modifiers::default()\n    };\n    visual.simulate_mouse_down(midpoint, MouseButton::Left, command);\n    visual.simulate_mouse_up(midpoint, MouseButton::Left, command);')
assert a==b;proof['all_five_old_test_bodies_preserved_except_requested_command_click_mapping']=True
for rev in [base,prior,integrated,'c71e52263ba3941b877381ad121a3a3ca10ba484','b95994b392575fd8cb3ac9581fdd3e87e2ad98d4']:
    subprocess.run(['git','merge-base','--is-ancestor',rev,source],check=True)
proof['preserved_ancestors']=[base,prior,integrated,'c71e52263ba3941b877381ad121a3a3ca10ba484','b95994b392575fd8cb3ac9581fdd3e87e2ad98d4']
(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(dest/'source-identities.json').write_text(json.dumps(identities,indent=2)+'\n')
for label,left,right in [('source',base,source),('continuation',integrated,source),('original-feature','296b9b46c7d5ded8b60e40ebeef1c435dc355115',prior),('original-diagnostic','296b9b46c7d5ded8b60e40ebeef1c435dc355115','c71e52263ba3941b877381ad121a3a3ca10ba484')]:
    (dest/(label+'.patch')).write_bytes(subprocess.check_output(['git','diff','--binary',left,right]))
(dest/'source-history.txt').write_bytes(subprocess.check_output(['git','log','--first-parent','--format=fuller',prior+'..'+source]))
for path in ['/tmp/header-component.py','/tmp/seal-329-loop.py']:
    d=dest/'orchestration';d.mkdir(exist_ok=True);shutil.copy2(path,d/Path(path).name)
(dest/'HANDOFF.md').write_text(Path('/tmp/329-loop-handoff.md').read_text())
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()};(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n');archive=dest/('hex329-loop-'+source[:8]+'-evidence.tar.gz')
with tarfile.open(archive,'w:gz') as tf:
    for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
    assert len(tf.getmembers())==len(manifest)+1
    for m in tf.getmembers():
        assert m.isfile() and not m.name.startswith('/') and '..' not in Path(m.name).parts
        if m.name in manifest:assert hashlib.sha256(tf.extractfile(m).read()).hexdigest()==manifest[m.name]['sha256']
with tempfile.TemporaryDirectory() as d:
    env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')};subprocess.run(['git','read-tree','--empty'],env=env,check=True);subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True);tree=git('write-tree',env=env).decode();commit=subprocess.check_output(['git','commit-tree',tree,'-m','Seal canonical PR329 continuation, original history and exact component evidence'],env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode(),'base':base,'prior':prior,'integrated_before':integrated,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1,'summary_sha256':sha(p),'seconds':s['duration_seconds']};(root/'artifacts/329-loop-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:assert subprocess.check_output(['git','show',commit+':'+str((dest/name).relative_to(root))])==(dest/name).read_bytes()
print(json.dumps(state,indent=2),flush=True);subprocess.run(['git','push','origin',ref],check=True)
remote=git('ls-remote','origin','refs/heads/'+ref,'refs/heads/pulse/task-2713-g1-58fda6b4d4c7').decode();assert commit in remote and source in remote
print('ALL payloads/Git bytes/archive/remote refs verified',flush=True)
