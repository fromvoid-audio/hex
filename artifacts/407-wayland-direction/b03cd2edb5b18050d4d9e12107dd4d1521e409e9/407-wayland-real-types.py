from pathlib import Path
import subprocess,json,hashlib,os,sys
from types import SimpleNamespace
r=Path('/workspace/hex');d=r/'artifacts/407-wayland-direction'
args=['cargo','test','-p','hex-ui','--lib','--features','ui-test-support','--locked','--message-format=json','node_gesture_tests::','--','--nocapture']
if '--reuse-build' in sys.argv:
 p=SimpleNamespace(stdout=(d/'workspace-build-and-node-controls.log').read_text(),returncode=0)
else:
 p=subprocess.run(args,cwd=r,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
(d/'workspace-build-and-node-controls.log').write_text(p.stdout)
print(p.stdout,flush=True)
if p.returncode:raise SystemExit(p.returncode)
libs={}
for line in p.stdout.splitlines():
 try:j=json.loads(line)
 except ValueError:continue
 if j.get('reason')=='compiler-artifact' and j['target']['name'] in ['gpui','wayland_client']:
  for f in j['filenames']:
   if f.endswith('.rlib'):libs[j['target']['name']]=f
assert set(libs)=={'gpui','wayland_client'},libs
s=(r/'third_party/gpui/src/platform/linux/wayland/client.rs').read_text();start=s.index('// Axis metadata stays local');end=s.index('pub(crate) struct WaylandClientState {',start);test=s[s.index('#[cfg(test)]\nmod axis_frame_tests {'):]
constant=next(line.replace('pub(crate) ', '') for line in (r/'third_party/gpui/src/platform/linux.rs').read_text().splitlines() if 'const SCROLL_LINES:' in line) if False else next(line.replace('pub(crate) ', '') for line in (r/'third_party/gpui/src/platform/linux/platform.rs').read_text().splitlines() if 'const SCROLL_LINES:' in line)
if '--reuse-build' in sys.argv:
 previous=json.loads((d/'real-types-provenance.json').read_text())
 assert previous['exact_fold_sha256']==hashlib.sha256(s[start:end].encode()).hexdigest(), 'cannot reuse differently compiled production fold'
unit='use gpui::{Point, Pixels, point, px, ScrollDelta, ScrollWheelEvent};\nuse wayland_client::{protocol::wl_pointer, protocol::wl_pointer::AxisSource, WEnum};\n'+constant+'\n'+s[start:end]+test+(d/'legacy-raw-differential.rs').read_text()
f=d/'production-fold-real-protocol-tests.rs';f.write_text(unit)
cmd=['rustc','--edition=2024','-D','warnings','--test',str(f),'-L','dependency='+str(Path(libs['gpui']).parent),'-o',str(d/'real-protocol-tests')]
for name,path in libs.items():cmd+=['--extern',name+'='+path]
for line in p.stdout.splitlines():
 try:j=json.loads(line)
 except ValueError:continue
 if j.get('reason')=='build-script-executed':
  for path in j.get('linked_paths',[]):cmd+=['-L',path]
(d/'real-types-provenance.json').write_text(json.dumps({'source_client_sha256':hashlib.sha256(s.encode()).hexdigest(),'exact_fold_sha256':hashlib.sha256(s[start:end].encode()).hexdigest(),'exact_tests_sha256':hashlib.sha256(test.encode()).hexdigest(),'compile_argv':cmd,'workspace_argv':args,'limits':'actual production-used helper/test bytes and actual gpui/Wayland types; no Dispatch connection/window/compositor/hardware'},indent=2)+'\n')
c=subprocess.run(cmd,cwd=r,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True);(d/'real-protocol-compile.log').write_text(c.stdout);print(c.stdout,flush=True)
if c.returncode:raise SystemExit(c.returncode)
t=subprocess.run([str(d/'real-protocol-tests'),'--nocapture'],cwd=r,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True);(d/'real-protocol-tests.log').write_text(t.stdout);print(t.stdout,flush=True);raise SystemExit(t.returncode)
