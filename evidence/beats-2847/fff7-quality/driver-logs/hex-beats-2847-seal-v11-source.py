from pathlib import Path
import json,hashlib,tarfile,shutil,subprocess,csv
root=Path('/dev/shm/hex-beats-2847-v11-source-packet');root.mkdir()
def cp(p,name):
 dst=root/name;dst.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(p,dst)
for name in ['hex-beats-2847-screen-final-cohort.log','hex-beats-2847-screen-final-clippy.log','hex-beats-2847-screen-search-oracles.log','hex-beats-2847-screen-engine.log','hex-beats-2847-screen-export.log','hex-beats-2847-screen-v11-matrix.log','hex-beats-2847-screen-cohort-first.log','hex-beats-2847-screen-clippy-first.log','hex-beats-2847-screen-bass-first.log','hex-beats-2847-screen-first-timpani.log','hex-beats-2847-screen-cache-build.log','hex-beats-2847-v11-fixed-render.log','hex-beats-2847-v11-auto-render.log','hex-beats-2847-v11-phrase-after.log','hex-beats-2847-c9f-phrase-before.log','hex-beats-2847-c9f-phrase-after.log','hex-beats-2847-v11-real-before-BKCtbss_SusNV_A1_v1_rr1.log','hex-beats-2847-v11-real-before-adapter-build.log','hex-beats-2847-bass-after-render.log','hex-beats-2847-v11-listening-packet.log']:
 p=Path('/tmp')/name
 if p.exists():cp(p,'logs/'+name)
for part in ['fixed','auto']:
 for p in Path('/dev/shm/hex-beats-2847-v11-'+part+'-render').iterdir():cp(p,'synthetic-'+part+'/'+p.name)
rows=[]
for p in Path('/dev/shm').glob('hex-beats-2847-screen-v11-*/metrics.csv'):
 cp(p,'real-default54/'+p.parent.name+'/'+p.name)
 for name in ['settings.txt','identity.json']:cp(p.parent/name,'real-default54/'+p.parent.name+'/'+name)
 r=list(csv.DictReader(p.open()));assert len(r)==9 and all(x['result']=='ok' for x in r);rows+=r
assert len(rows)==54
for name in ['hex-beats-2847-screen-v11-matrix.py','hex-beats-2847-build-phrase.py','hex-beats-2847-v11-listening-packet.py','hex-beats-2847-v11-listening-packet-resume.py','hex-beats-2847-baseline-recording-adapter-original.rs']:
 cp(Path('/dev/shm')/name,'evidence-tools/'+name)
cp(Path('/dev/shm/hex-beats-declick/crates/hex-warp/examples/beats_recording.rs'),'evidence-tools/current-identical-recording-adapter.rs')
(root/'source-change.patch').write_bytes(subprocess.check_output(['git','diff','c9f4b8864189d968a8a19abcc525c9725755e43f','fff77efcdbe22c67e574e2e6d16bf3f2454d943e'],cwd='/dev/shm/hex-beats-declick'))
(root/'PROVENANCE.json').write_text(json.dumps(dict(source='fff77efcdbe22c67e574e2e6d16bf3f2454d943e',tree='3142d295a4522ca5d36e850c614e1d43c85810e2',algorithm_version=11,focused_tests=43,failed=0,ignored=0,engine_tests=3,export_tests=1,strict_warp_alltargets_clippy='PASS',recording_renders=54,work_limit=1000000000,max_recording_work=max(int(r['work']) for r in rows),max_recording_scratch=max(int(r['scratch']) for r in rows),readiness='focused source evidence; final integrated CI/native/listening/mixed-music quality acceptance open',old_failures='earlier immutable packets; original 2 timpani WorkLimit not relabelled; c9f sub-bass RED retained in c9f-groups packet',packaging_history='initial optional-case adapter invocation rejected before any render; copied same final evidence-only adapter into baseline9c and rebuilt, then only previously unrendered four original2xForward cases. Initial listening package stopped after WAV generation on an incorrect original-WAV filename; resume verifies all existing PCM24 bytes rather than rendering DSP again. Both raw packaging/driver logs preserved.'),indent=2)+'\n')
files=[]
for p in sorted(root.rglob('*')):
 if p.is_file():files.append(dict(path=str(p.relative_to(root)),bytes=p.stat().st_size,sha256=hashlib.sha256(p.read_bytes()).hexdigest()))
(root/'manifest.json').write_text(json.dumps(files,indent=2)+'\n')
out=Path('/dev/shm/hex-beats-2847-v11-source.tar.gz')
with tarfile.open(out,'w:gz',compresslevel=3) as t:t.add(root,arcname='v11-source')
print(out.stat().st_size,hashlib.sha256(out.read_bytes()).hexdigest(),len(files),flush=True)
