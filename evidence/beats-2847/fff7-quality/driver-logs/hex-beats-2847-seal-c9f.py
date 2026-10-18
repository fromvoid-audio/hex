from pathlib import Path
import shutil,hashlib,json,tarfile,subprocess,struct,wave
root=Path('/dev/shm/hex-beats-2847-c9f-group-packet');root.mkdir()
def cp(src,dst):
 dst=root/dst;dst.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(src,dst)
for p in Path('/dev/shm/hex-beats-2847-c9f-fixed-render').iterdir():cp(p,'fixed-grid-intermediate/'+p.name)
for p in Path('/dev/shm').glob('hex-beats-2847-c9f-*/metrics.csv'):cp(p,'recording-default54/'+p.parent.name+'/'+p.name)
for p in Path('/dev/shm').glob('hex-beats-2847-c9f-*/identity.json'):cp(p,'recording-default54/'+p.parent.name+'/'+p.name)
patterns=['hex-beats-2847-group*.log','hex-beats-2847-fixed-groups*.log','hex-beats-2847-pruning-exhaustive*.log','hex-beats-2847-c9f-bass-band-first.log','hex-beats-2847-bass-width*.log','hex-beats-2847-bass-before-render.log','hex-beats-2847-c9f-matrix.log','hex-beats-2847-c9f-fixed-render.log']
for pattern in patterns:
 for p in Path('/tmp').glob(pattern):cp(p,'logs/'+p.name)
for p in Path('/dev/shm').glob('hex-beats-2847-bass-width*.patch'):cp(p,'diagnostic-patches/'+p.name)
for p in Path('/dev/shm/hex-beats-2847-bass-c9f-before').rglob('*'):
 if p.is_file():cp(p,'sub-bass-red/'+str(p.relative_to('/dev/shm/hex-beats-2847-bass-c9f-before')))
(root/'source-change.patch').write_bytes(subprocess.check_output(['git','diff','86ea559eff7328e27597b9d6185f809441278d68','c9f4b8864189d968a8a19abcc525c9725755e43f'],cwd='/dev/shm/hex-beats-declick'))
(root/'PROVENANCE.txt').write_text('INTERMEDIATE SOURCE, NOT FINAL READINESS. c9f4b8864189d968a8a19abcc525c9725755e43f/tree7d61e28c32a3d20f9810b545818cc56a527b51ef version10. Seven fixed Preserve A timing tests,480 full-Match exact pruning oracle,41 focused tests/Engine3/Export1/strict Clippy passed. All54 default acoustic renders actually completed and were byte-identical to86ea. Fixed-grid80 WAVs have their own version10 settings. New27.5Hz RED is genuine; width-only diagnostic source was restored before next product change. Its final diagnostic Invalid was unsupported compensated8x/+307 fixture, not renderer failure; final test asserts six such negatives separately. Earlier red fixtures and intermediate control failures are preserved as raw logs, not relabelled passing. Original source9c1a/before80 and97bae remain separate immutable packets. No actual listening/Live reference/native/app actions.\n')
files=[]
for p in sorted(root.rglob('*')):
 if p.is_file():files.append(dict(path=str(p.relative_to(root)),bytes=p.stat().st_size,sha256=hashlib.sha256(p.read_bytes()).hexdigest()))
(root/'manifest.json').write_text(json.dumps(files,indent=2)+'\n')
out=Path('/dev/shm/hex-beats-2847-c9f-groups.tar.gz')
with tarfile.open(out,'w:gz',compresslevel=3) as t:t.add(root,arcname='c9f-groups')
print(out.stat().st_size,hashlib.sha256(out.read_bytes()).hexdigest(),len(files),flush=True)
