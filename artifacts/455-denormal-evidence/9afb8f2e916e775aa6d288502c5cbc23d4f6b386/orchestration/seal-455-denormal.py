import hashlib,json,os,shutil,subprocess,tarfile,tempfile
from pathlib import Path
root=Path('/workspace/hex');os.chdir(root)
def git(*args,env=None):return subprocess.check_output(['git',*args],env=env)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source='9afb8f2e916e775aa6d288502c5cbc23d4f6b386';tested='edbef1d829084181905c5f14a0abe0e37f88b676';base='99e7f5eb0cddd8e298cc8533888d48341a88d3f2';ref='evidence/455-denormal-9afb8f2e-20261019';dest=root/'artifacts/455-denormal-evidence'/source
assert git('rev-parse','HEAD').decode().strip()==source and not git('status','--porcelain') and not dest.exists()
receipts={}
for stage,commit in [('final',tested),('final-tone',source)]:
 p=root/('artifacts/local-ci/455-denormal-'+stage)/'summary.json';s=json.loads(p.read_text());assert s['exit_code']==0 and not s['stale'] and s['start_tree']==s['end_tree'] and not s['start_tree']['dirty'] and s['start_tree']['commit']==commit;receipts[stage]={'source':commit,'seconds':s['duration_seconds'],'summary_sha256':sha(p),'content_sha256':s['start_tree']['content_sha256']}
dest.mkdir(parents=True)
for stage in ['red','green','final','final-tone']:
 shutil.copytree(root/('artifacts/local-ci/455-denormal-'+stage),dest/'runs'/stage);shutil.copy2(root/('artifacts/455-denormal-'+stage+'-run.log'),dest/(stage+'-outer.log'))
for name in ['Cargo.toml','Cargo.lock','probe.rs']:shutil.copy2(root/'artifacts/455-denormal-allocation'/name,dest/name)
files=git('diff','--name-only',base,source).decode().splitlines();assert files==['crates/hex-devices/src/compressor.rs','crates/hex-devices/src/compressor/tests.rs','docs/compressor.md'];ids={}
for name in files:
 ids[name]={}
 for label,rev in [('before',base),('after',source),('full-targeted-tested',tested)]:
  b=git('show',rev+':'+name);q=dest/'source'/label/name;q.parent.mkdir(parents=True,exist_ok=True);q.write_bytes(b);ids[name][label]={'git_blob':git('rev-parse',rev+':'+name).decode().strip(),'sha256':hashlib.sha256(b).hexdigest()}
 assert (root/name).read_bytes()==git('show',source+':'+name)
a=git('show',base+':'+files[0]).decode();b=git('show',source+':'+files[0]).decode()
assert b.replace('// Power floor: sqrt(1e-30) is -300 dBFS, far below the useful detector\n// range. Retire silent tails before f64 subnormals can persist in warm history.\nconst RMS_ENERGY_FLOOR: f64 = 1e-30;\n','').replace('            if *energy < RMS_ENERGY_FLOOR {\n                *energy = 0.0;\n            }\n','')==a
assert git('show',source+':'+files[0])==git('show',tested+':'+files[0])
assert git('show',source+':'+files[1]).startswith(git('show',base+':'+files[1]))
assert git('show',source+':'+files[2]).startswith(git('show',base+':'+files[2]))
assert set(git('diff','--name-only',tested,source).decode().splitlines())==set(files[1:])
proof={'production_delta_exact_six_additive_lines_reverse_applies_byte_exact':True,'all_original_provider_tests_and_docs_prefix_unchanged':True,'full_targeted_edbef_production_blob_equals_final9afb':True,'edbef_to_final_only_tests_docs':files[1:]}
for name in ['AGENTS.md','scripts/local-ci.py','Cargo.toml','Cargo.lock','crates/hex-ui/src/editor/device_panel/compressor_panel.rs','crates/hex-ui/src/editor/device_panel/tests/compressor_ui_tests.rs','crates/hex-ui/src/editor/device_panel/tests/device_comparison_tests.rs','crates/hex-audio/src/prepared.rs','crates/hex-engine/src/lib.rs','crates/hex-session/src/lib.rs','crates/hex-storage/src/lib.rs']:
 assert git('show',base+':'+name)==git('show',source+':'+name)==git('show',tested+':'+name);proof[name]=hashlib.sha256(git('show',source+':'+name)).hexdigest()
(dest/'source-identities.json').write_text(json.dumps(ids,indent=2)+'\n');(dest/'protected-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(dest/'source.patch').write_bytes(git('diff','--binary',base,source));(dest/'source-history.txt').write_bytes(git('log','--format=fuller',base+'..'+source));(dest/'HANDOFF.md').write_text(Path('/tmp/455-denormal-handoff.md').read_text()+'\n## Source-bound exact receipts\n\n'+json.dumps(receipts,indent=2)+'\n')
for name in ['/tmp/455-denormal-component.py','/tmp/seal-455-denormal.py','/workspace/hex/artifacts/310-approved-guard-runner.py']:
 q=dest/'orchestration'/Path(name).name;q.parent.mkdir(exist_ok=True);shutil.copy2(name,q)
manifest={q.relative_to(dest).as_posix():{'size':q.stat().st_size,'sha256':sha(q)} for q in sorted(dest.rglob('*')) if q.is_file()};(dest/'MANIFEST.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n');archive=dest/'hex455-denormal-9afb8f2e-evidence.tar.gz'
with tarfile.open(archive,'w:gz') as tf:
 for name in [*manifest,'MANIFEST.json']:tf.add(dest/name,arcname=name,recursive=False)
with tarfile.open(archive) as tf:
 assert len(tf.getmembers())==len(manifest)+1
 for m in tf.getmembers():
  assert m.isfile() and not m.name.startswith('/') and '..' not in Path(m.name).parts
  if m.name in manifest:assert hashlib.sha256(tf.extractfile(m).read()).hexdigest()==manifest[m.name]['sha256']
with tempfile.TemporaryDirectory() as d:
 env={**os.environ,'GIT_INDEX_FILE':str(Path(d)/'index')};subprocess.run(['git','read-tree','--empty'],env=env,check=True);subprocess.run(['git','add','-f','--',str(dest.relative_to(root))],env=env,check=True);tree=git('write-tree',env=env).decode().strip();commit=git('commit-tree',tree,'-m','Seal PR455 energy retirement source and finite actual evidence',env=env).decode().strip()
subprocess.run(['git','update-ref','refs/heads/'+ref,commit],check=True)
state={'source':source,'tree':git('rev-parse',source+'^{tree}').decode().strip(),'base':base,'receipts':receipts,'evidence':commit,'evidence_tree':tree,'ref':ref,'folder':str(dest.relative_to(root)),'archive':archive.name,'archive_size':archive.stat().st_size,'archive_sha256':sha(archive),'manifest_sha256':sha(dest/'MANIFEST.json'),'payloads':len(manifest),'safe_members':len(manifest)+1};(root/'artifacts/455-denormal-sealed-state.json').write_text(json.dumps(state,indent=2)+'\n')
for name in [*manifest,'MANIFEST.json',archive.name]:assert git('show',commit+':'+str((dest/name).relative_to(root)))==(dest/name).read_bytes()
print(json.dumps(state,indent=2),flush=True)
