#!/usr/bin/env python3
"""Offline builder for ORIGINAL descriptor and offline CLAP Linux fixtures; no installation/fetch."""
import argparse,hashlib,json,platform,subprocess,shutil,re
from pathlib import Path
PIN='195b42a004144fab0b3cf95e9c067187d15365b7'
NATIVE=Path(__file__).resolve().parent
NAMES=['good','init-fail','no-factory','duplicate','malformed','api-bad','crash','hang','descendant','deinit-hang','bad-utf8','too-many','unclosed-features']
OFFLINE=['process','process-hang','process-crash','process-nonfinite','process-error','process-descendant','instance-init-fail','state-fail','bus-refuse','restart','sleep','tail']
MIDI=['midi','midi-sleep','midi-error','midi-crash','midi-hang','midi-descendant','midi-rescan','midi-bad-port','midi-bad-output','midi-unsorted-output','midi-block-overflow','midi-total-overflow','midi-wrong-thread','midi-state-fail','midi-output-thread','midi-late-output']
DELAY=['delay-0','delay-1','delay-16','delay-31','delay-32','delay-inactive-callback','delay-thread-callback','delay-drift','delay-restart','delay-infinite-tail','delay-shrink-tail','delay-late-error','delay-drain-hang','delay-drain-crash','delay-descendant','delay-late-nonfinite','delay-active-callback','delay-main-tail','delay-large-tail','delay-stop-tail','delay-late-callback','delay-ignored-output','delay-late-output','delay-foreign-output']
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def source_digest():
 h=hashlib.sha256()
 for name in ['fixture.cpp','probe.cpp']:h.update(name.encode()+b'\0'+(NATIVE/name).read_bytes()+b'\0')
 return h.hexdigest()
def main():
 parser=argparse.ArgumentParser();parser.add_argument('--sdk',type=Path,required=True);parser.add_argument('--output',type=Path,required=True);a=parser.parse_args()
 if platform.system()!='Linux' or platform.machine()!='x86_64':raise SystemExit('Only Linux x86_64 controlled fixtures supported')
 sdk=a.sdk.resolve();out=a.output.resolve();out.mkdir(parents=True,exist_ok=True)
 if subprocess.check_output(['git','rev-parse','HEAD'],cwd=sdk,text=True).strip()!=PIN:raise SystemExit('CLAP source pin mismatch')
 if subprocess.check_output(['git','status','--porcelain','--untracked-files=no'],cwd=sdk):raise SystemExit('CLAP tracked source dirty')
 license_path=sdk/'LICENSE';license_text=license_path.read_text()
 if 'MIT License' not in license_text:raise SystemExit('Unreviewed CLAP license')
 if license_text!=(NATIVE/'CLAP-LICENSE.txt').read_text():raise SystemExit('Retained CLAP notice mismatch')
 commands=[];closure={};artifacts=[]
 def build(source,target,extra):
  cmd=['c++','-std=c++17','-O2','-Wall','-Wextra','-Werror','-I'+str(sdk/'include'),'-MD','-MF',str(target.with_suffix('.d')),str(NATIVE/source),'-o',str(target)]+extra
  commands.append(cmd);subprocess.run(cmd,check=True);artifacts.append(target)
  deps=target.with_suffix('.d').read_text().replace('\\\n',' ').split(':',1)[1].split()
  for name in deps:
   p=Path(name).resolve()
   if p.is_relative_to(sdk):key='clap/'+str(p.relative_to(sdk))
   elif p.is_relative_to(NATIVE):key='native/'+str(p.relative_to(NATIVE))
   elif any(p.is_relative_to(Path(r))for r in ['/usr/include','/usr/lib/gcc','/usr/lib/linux/uapi']):key='system/'+str(p)
   else:raise SystemExit('Unreviewed build input: '+name)
   closure[key]=sha(p)
 build('probe.cpp',out/'hex-clap-probe',['-ldl'])
 build('launch_ignored.cpp',out/'launch-ignored',[])
 modules=[]
 for mode,name in enumerate(NAMES):
  target=out/(name+'.clap');build('fixture.cpp',target,['-shared','-fPIC','-Wl,--no-undefined','-DFIXTURE_MODE='+str(mode)])
  modules.append({'name':name,'binary':target.name,'sha256':sha(target)})
 build('offline.cpp',out/'hex-clap-offline-helper',['-ldl','-pthread'])
 offline_modules=[]
 for mode,name in enumerate(OFFLINE):
  target=out/(name+'.clap');build('processing_fixture.cpp',target,['-shared','-fPIC','-Wl,--no-undefined','-pthread','-DFIXTURE_MODE='+str(mode)])
  offline_modules.append({'name':name,'binary':target.name,'sha256':sha(target)})
 for mode,name in enumerate(MIDI):
  target=out/(name+'.clap');build('midi_fixture.cpp',target,['-shared','-fPIC','-Wl,--no-undefined','-pthread','-DFIXTURE_MODE='+str(mode)])
  offline_modules.append({'name':name,'binary':target.name,'sha256':sha(target)})
 for index,name in enumerate(DELAY):
  delay=[0,1,16,31,32][index] if index<5 else 16
  mode=0 if index<5 else index-4
  target=out/(name+'.clap');build('delay_fixture.cpp',target,['-shared','-fPIC','-Wl,--no-undefined','-pthread','-DFIXTURE_MODE='+str(mode),'-DFIXTURE_DELAY='+str(delay)])
  offline_modules.append({'name':name,'binary':target.name,'sha256':sha(target)})
 runtime={}
 # Only ORIGINAL freshly built binaries are inspected; never ldd untrusted modules.
 for target in artifacts:
  for line in subprocess.check_output(['ldd',str(target)],text=True).splitlines():
   match=re.search(r'(/[^ ]+)',line)
   if match:p=Path(match.group(1)).resolve();runtime[str(p)]=sha(p)
 notices={}
 for name in ['libstdc++6','libgcc-s1','libc6','linux-libc-dev']:
  p=Path('/usr/share/doc')/name/'copyright';notices[name]={'sha256':sha(p),'text':p.read_text()}
 r={'version':1,'scope':'original-controlled-clap-fixtures','platform':'linux-x86_64','sdk':PIN,'native_source_sha256':source_digest(),'helper_sha256':sha(out/'hex-clap-probe'),'launcher_sha256':sha(out/'launch-ignored'),'modules':modules,'compiler':subprocess.check_output(['c++','--version'],text=True).splitlines()[0],'toolchain_sha256':sha(Path(shutil.which('c++')).resolve()),'runtime_closure':runtime,'runtime_notices':notices,'build_closure':closure,'licenses':{'clap':{'sha256':sha(license_path),'text':license_text}}}
 p=out/'controlled-clap-fixtures.json';p.write_text(json.dumps(r,indent=2)+'\n');
 h=hashlib.sha256()
 for name in ['offline.cpp','offline_host.hpp','offline_wire.hpp','processing_fixture.cpp','midi_fixture.cpp','delay_attempt.hpp','delay_wire.hpp','delay_fixture.cpp']:h.update(name.encode()+b'\0'+(NATIVE/name).read_bytes()+b'\0')
 offline={'version':1,'scope':'original-controlled-offline-fixtures','scanner_receipt_sha256':sha(p),'source_sha256':h.hexdigest(),'helper_sha256':sha(out/'hex-clap-offline-helper'),'modules':offline_modules}
 (out/'controlled-offline.json').write_text(json.dumps(offline,indent=2)+'\n');(out/'build-commands.json').write_text(json.dumps(commands,indent=2)+'\n');print('Controlled receipt:',p);print('SHA256:',sha(p));print('Artifacts:',len(artifacts),'headers:',len(closure),'runtime libraries:',len(runtime))
if __name__=='__main__':main()
