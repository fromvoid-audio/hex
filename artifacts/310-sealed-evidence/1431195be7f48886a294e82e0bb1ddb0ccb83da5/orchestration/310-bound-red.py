import subprocess,pathlib
r=subprocess.run(['cargo','test','--release','--manifest-path','/workspace/hex/artifacts/310-metadata-harness/bound-red/Cargo.toml','--target-dir','/workspace/hex/artifacts/310-private-target','adaptive_operation_work_profile','--','--nocapture'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
p=pathlib.Path('/workspace/hex/artifacts/310-original-work-bound-red.log');p.write_bytes(r.stdout)
assert r.returncode==101,r.stdout.decode()
assert b'paint: [1, 13, 2, 12]' in r.stdout and b'1 failed' in r.stdout,r.stdout.decode()
print('ACTUAL baseline work bound RED, exit101/1failed: paint [step1,base13,share2,demand12] exceeds once-per-section bound; retained raw log')
