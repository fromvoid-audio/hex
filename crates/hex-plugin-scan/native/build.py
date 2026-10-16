#!/usr/bin/env python3
"""Build only original controlled Linux factory fixtures, from pre-fetched pinned SDK."""
import argparse, hashlib, json, platform, subprocess, shutil, re
from pathlib import Path
PINS={'sdk':'3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96','base':'fcf9da0bd27a16f7f03773a3a39822f28f5c8477','cmake':'054c9143cbb8d47fc4694e473f2ee3b4d951a8f5','pluginterfaces':'4f547e8e102b47de4a8b8aaf343c73b700786372','public.sdk':'586dc5e6c8012c3e4b01c79389375cbe96bdb1da'}
NATIVE=Path(__file__).resolve().parent
ROOT=NATIVE.parents[2]
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def native_digest():
 h=hashlib.sha256()
 for name in ['fixture.cpp','probe.cpp']:
  h.update(name.encode()+b'\0'+(NATIVE/name).read_bytes()+b'\0')
 return h.hexdigest()
def main():
 parser=argparse.ArgumentParser();parser.add_argument('--sdk',type=Path,required=True);parser.add_argument('--output',type=Path,required=True);args=parser.parse_args()
 if platform.system()!='Linux' or platform.machine()!='x86_64':raise SystemExit('Only Linux x86_64 controlled fixtures supported')
 sdk=args.sdk.resolve();output=args.output.resolve();output.mkdir(parents=True,exist_ok=True)
 licenses={}
 for name,commit in PINS.items():
  path=sdk if name=='sdk' else sdk/name
  actual=subprocess.check_output(['git','rev-parse','HEAD'],cwd=path,text=True).strip()
  if actual!=commit:raise SystemExit('SDK pin mismatch: '+name)
  if subprocess.check_output(['git','status','--porcelain','--untracked-files=no'],cwd=path):raise SystemExit('SDK dirty: '+name)
  license_path=path/'LICENSE.txt'
  if not license_path.exists():license_path=path/'LICENSE'
  if 'MIT License' not in license_path.read_text():raise SystemExit('Unreviewed SDK license: '+name)
  licenses[name]={'sha256':sha(license_path),'text':license_path.read_text()}
 common=['c++','-std=c++17','-O2','-Wall','-Wextra','-Werror','-I'+str(sdk)]
 commands=[];closures={}
 def build(source,destination,extra):
  dependency=destination.with_suffix('.d')
  cmd=common+['-MD','-MF',str(dependency),str(NATIVE/source),'-o',str(destination)]+extra
  commands.append(cmd);subprocess.run(cmd,check=True)
  # MD includes compiler/standard headers. Audit the actual source closure.
  # Paths containing spaces are deliberately refused.
  deps=dependency.read_text().replace('\\\n',' ').split(':',1)[1].split()
  for name in deps:
   path=Path(name).resolve()
   if path.is_relative_to(sdk):key='sdk/'+str(path.relative_to(sdk))
   elif path.is_relative_to(NATIVE):key='native/'+str(path.relative_to(NATIVE))
   elif path.is_relative_to(Path('/usr/include')) or path.is_relative_to(Path('/usr/lib/gcc')) or path.is_relative_to(Path('/usr/lib/linux/uapi')):key='system/'+str(path)
   else:raise SystemExit('Unreviewed build input: '+name)
   closures[key]=sha(path)
 build('probe.cpp',output/'hex-vst3-probe',['-ldl'])
 build('launch_ignored.cpp',output/'launch-ignored',[])
 build('offline.cpp',output/'hex-vst3-offline-helper',['-ldl','-pthread'])
 offline_modules=[]
 for mode,name in enumerate(['process','process-hang','process-crash','process-nonfinite','process-error','process-descendant','initialize-fail','controller-fail','connect-fail','state-fail','bus-refuse','restart','unload-hang']):
  relative=Path(name+'.vst3')/'Contents'/'x86_64-linux'/(name+'.so');destination=output/relative;destination.parent.mkdir(parents=True,exist_ok=True)
  build('processing_fixture.cpp',destination,['-shared','-fPIC','-Wl,--no-undefined','-DFIXTURE_MODE='+str(mode)])
  offline_modules.append({'name':name,'binary':str(relative),'sha256':sha(destination)})
 modules=[]
 for name,mode in [('good',0),('hang',1),('crash',2),('descendant',3),('discardable',4)]:
  relative=Path(name+'.vst3')/'Contents'/'x86_64-linux'/(name+'.so');destination=output/relative;destination.parent.mkdir(parents=True,exist_ok=True)
  build('fixture.cpp',destination,['-shared','-fPIC','-Wl,--no-undefined','-DFIXTURE_MODE='+str(mode)])
  modules.append({'name':name,'binary':str(relative),'sha256':sha(destination)})
 runtime={}
 for artifact in [output/'hex-vst3-probe',output/'hex-vst3-offline-helper',output/'launch-ignored']+[output/m['binary'] for m in modules+offline_modules]:
  for line in subprocess.check_output(['ldd',str(artifact)],text=True).splitlines():
   match=re.search(r'(/[^ ]+)',line)
   if match:
    path=Path(match.group(1)).resolve();runtime[str(path)]=sha(path)
 runtime_notices={}
 for name in ['libstdc++6','libgcc-s1','libc6','linux-libc-dev']:
  path=Path('/usr/share/doc')/name/'copyright'
  runtime_notices[name]={'sha256':sha(path),'text':path.read_text()}
 receipt={'version':1,'scope':'original-controlled-sdk-fixtures','platform':'linux-x86_64','sdk':PINS['sdk'],'public_sdk':PINS['public.sdk'],'native_source_sha256':native_digest(),'helper_sha256':sha(output/'hex-vst3-probe'),'launcher_sha256':sha(output/'launch-ignored'),'modules':modules,'toolchain_sha256':sha(Path(shutil.which('c++')).resolve()),'runtime_closure':runtime,'runtime_notices':runtime_notices,'compiler':subprocess.check_output(['c++','--version'],text=True).splitlines()[0],'build_closure':closures,'licenses':licenses}
 receipt_path=output/'controlled-fixtures.json';receipt_path.write_text(json.dumps(receipt,indent=2)+'\n')
 h=hashlib.sha256()
 for name in ['offline.cpp','offline_host.hpp','offline_wire.hpp','processing_fixture.cpp']:
  h.update(name.encode()+b'\0'+(NATIVE/name).read_bytes()+b'\0')
 offline={'version':1,'scope':'original-controlled-offline-fixtures','scanner_receipt_sha256':sha(receipt_path),'source_sha256':h.hexdigest(),'helper_sha256':sha(output/'hex-vst3-offline-helper'),'modules':offline_modules}
 (output/'controlled-offline.json').write_text(json.dumps(offline,indent=2)+'\n')
 (output/'build-commands.json').write_text(json.dumps(commands,indent=2)+'\n')
 print('Controlled fixture receipt:',receipt_path);print('Receipt SHA256:',sha(receipt_path))
if __name__=='__main__':main()
