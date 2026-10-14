#!/usr/bin/env python3
"""Operator-only ORIGINAL never-fork builder. Does not enroll Rust tools or load modules."""
import argparse, ast, hashlib, json, os, platform, shlex, subprocess
from pathlib import Path
PIN='195b42a004144fab0b3cf95e9c067187d15365b7'
NATIVE=Path(__file__).resolve().parent
REPO=NATIVE.parents[2]
DESCRIPTOR_SOURCE=['fixture.cpp','probe.cpp','platform_identity.hpp','macos_runtime.hpp']
OFFLINE_SOURCE=['offline.cpp','offline_host.hpp','offline_wire.hpp','processing_fixture.cpp','midi_fixture.cpp','delay_attempt.hpp','delay_wire.hpp','delay_fixture.cpp','platform_identity.hpp','macos_runtime.hpp']
def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def source_hash(names):
    h=hashlib.sha256()
    for n in names:h.update(n.encode()+b'\0'+(NATIVE/n).read_bytes()+b'\0')
    return h.hexdigest()
def query(args,cwd=None,env=None):return subprocess.check_output(args,cwd=cwd,env=env,text=True,timeout=10).strip()
def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--sdk',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
    p.add_argument('--llvm-notice',type=Path,required=True);p.add_argument('--apple-sdk-notice',type=Path,required=True)
    p.add_argument('--llvm-origin',required=True);p.add_argument('--apple-sdk-origin',required=True)
    a=p.parse_args()
    if platform.system()!='Darwin' or platform.machine()!='arm64':raise SystemExit('Actual arm64 Mac required; no translated/foreign admission')
    if query(['git','status','--porcelain'],REPO):raise SystemExit('Exact clean committed source required')
    tree=query(['git','rev-parse','HEAD^{tree}'],REPO)
    sdk=a.sdk.resolve(strict=True);out=a.output.resolve()
    if out.exists() or out.is_relative_to(REPO) or out.is_relative_to(sdk):raise SystemExit('New operator artifact directory outside source/SDK required')
    if query(['git','rev-parse','HEAD'],sdk)!=PIN or query(['git','status','--porcelain','--untracked-files=no'],sdk):raise SystemExit('CLAP pin/source refused')
    if (sdk/'LICENSE').read_bytes()!=(NATIVE/'CLAP-LICENSE.txt').read_bytes():raise SystemExit('CLAP notice changed')
    clang=Path(query(['xcrun','--find','clang++'])).resolve(strict=True)
    sysroot=Path(query(['xcrun','--sdk','macosx','--show-sdk-path'])).resolve(strict=True)
    toolroot=clang.parents[2]
    for path in [REPO,sdk,out,sysroot,toolroot]:
        if ':' in str(path):raise SystemExit('Colon path unsupported in depfile audit')
    notices={}
    for name,path,origin in [('clap',sdk/'LICENSE','https://github.com/free-audio/clap'),('llvm-libcxx',a.llvm_notice,a.llvm_origin),('apple-sdk',a.apple_sdk_notice,a.apple_sdk_origin)]:
        path=path.resolve(strict=True)
        if not path.is_file() or path.stat().st_size>512*1024 or not origin or len(origin)>1024:raise SystemExit('Explicit operator-audited notice required')
        notices[name]=(path,origin)
    modes={}
    for node in ast.parse((NATIVE/'build.py').read_text()).body:
        if isinstance(node,ast.Assign):
            for target in node.targets:
                if isinstance(target,ast.Name) and target.id in ['NAMES','OFFLINE']:modes[target.id]=ast.literal_eval(node.value)
    if len(modes['NAMES'])!=13 or len(modes['OFFLINE'])!=12:raise SystemExit('Original mode inventory changed')
    out.mkdir(parents=True,mode=0o700)
    if out.stat().st_mode&0o022:raise SystemExit('Operator artifact root must not be group/other writable')
    commands=[];closure={};modules=[];helpers={};runtimes={}
    def build(source,binary,flags):
        target=out/binary;dep=out/(binary+'.d')
        cmd=[str(clang),'-std=c++17','-O2','-Wall','-Wextra','-Werror','-arch','arm64','-isysroot',str(sysroot),'-I'+str(sdk/'include'),'-MD','-MF',str(dep),str(NATIVE/source),'-o',str(target)]+flags
        log=out/(binary+'.build.log')
        with log.open('wb')as stream:r=subprocess.run(cmd,stdout=stream,stderr=subprocess.STDOUT,timeout=60)
        commands.append({'argv':cmd,'exit_code':r.returncode,'log_sha256':sha(log)})
        (out/'build-commands.json').write_text(json.dumps(commands,indent=2)+'\n')
        if r.returncode:raise SystemExit('Strict native build failed: '+str(log))
        for name in shlex.split(dep.read_text().replace('\\\n',' ').split(':',1)[1]):
            path=Path(name).resolve(strict=True)
            for prefix,root in [('clap',sdk),('native',NATIVE),('apple-sdk',sysroot),('apple-toolchain',toolroot)]:
                if path.is_relative_to(root):closure[prefix+'/'+str(path.relative_to(root))]=sha(path);break
            else:raise SystemExit('Unreviewed build closure input: '+str(path))
        # Original compiler outputs only, inspected without module execution.
        for index,args in enumerate([['xcrun','otool','-L',str(target)],['xcrun','otool','-l',str(target)],['/usr/bin/file',str(target)]]):
            log=out/(binary+'.identity-'+str(index)+'.inspect.log')
            with log.open('wb')as stream:r=subprocess.run(args,stdout=stream,stderr=subprocess.STDOUT,timeout=10)
            commands.append({'argv':args,'exit_code':r.returncode,'log_sha256':sha(log)})
            if r.returncode:raise SystemExit('Native image inspection failed')
        return sha(target)
    for source,binary in [('probe.cpp','hex-clap-probe'),('offline.cpp','hex-clap-offline-helper'),('launch_ignored.cpp','launch-ignored')]:
        helpers[binary]=build(source,binary,[])
    for key,source,excluded in [('NAMES','fixture.cpp','descendant'),('OFFLINE','processing_fixture.cpp','process-descendant')]:
        for mode,name in enumerate(modes[key]):
            if name==excluded:continue
            digest=build(source,name+'.clap',['-dynamiclib','-Wl,-undefined,error','-DFIXTURE_MODE='+str(mode)])
            modules.append({'name':name,'binary':name+'.clap','sha256':digest})
    # This mode enumerates already-loaded system image UUIDs and immediately exits;
    # it does NOT dlopen a fixture or process PCM. No dyld-cache hash is invented.
    for helper in ['hex-clap-probe','hex-clap-offline-helper']:
        args=[str(out/helper),'--runtime-identity'];raw=query(args,env={'LANG':'C'})
        identity=json.loads(raw)
        if identity.get('kind')!='loaded-image-uuid-v1' or identity.get('shared_cache_uuid')is not None:raise SystemExit('Runtime identity unavailable')
        runtimes[helper]=identity;commands.append({'argv':args,'exit_code':0,'identity_sha256':hashlib.sha256(raw.encode()).hexdigest()})
    retained={}
    for name,(path,origin)in notices.items():
        file='notice-'+name+'.txt';(out/file).write_bytes(path.read_bytes());retained[name]={'file':file,'sha256':sha(out/file),'origin':origin}
    if query(['git','status','--porcelain'],REPO) or query(['git','rev-parse','HEAD^{tree}'],REPO)!=tree:raise SystemExit('Source changed during build')
    if query(['git','status','--porcelain','--untracked-files=no'],sdk) or query(['git','rev-parse','HEAD'],sdk)!=PIN:raise SystemExit('SDK changed during build')
    commands_file=out/'build-commands.json';commands_file.write_text(json.dumps(commands,indent=2)+'\n')
    receipt={'version':2,'scope':'trusted-original-never-fork-v1','platform':'macos-arm64','sdk':PIN,'source_tree':tree,
             'descriptor_source_sha256':source_hash(DESCRIPTOR_SOURCE),'offline_source_sha256':source_hash(OFFLINE_SOURCE),
             'owner_uid':os.getuid(),'fixture_root':str(out),'compiler':str(clang),'compiler_version':query([str(clang),'--version']),
             'compiler_sha256':sha(clang),'apple_sdk_path':str(sysroot),'apple_sdk_version':query(['xcrun','--sdk','macosx','--show-sdk-version']),
             'build_commands_sha256':sha(commands_file),'build_closure':closure,'notices':retained,'helpers':helpers,'modules':modules,'runtimes':runtimes}
    file=out/'controlled-darwin.json';file.write_text(json.dumps(receipt,indent=2)+'\n')
    print('Operator artifact root:',out);print('Receipt SHA256:',sha(file));print('Source tree:',tree)
    print('Rust tools remain closed until operator build-time root/receipt/tree enrollment; no module was loaded.')
if __name__=='__main__':main()
