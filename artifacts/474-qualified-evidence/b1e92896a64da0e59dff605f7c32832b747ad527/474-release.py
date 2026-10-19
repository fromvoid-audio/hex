import importlib.util,sys,pathlib,json,subprocess,time
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_release_ci',root/'artifacts/310-approved-guard-runner.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m);m.ROOT=root
r=m.LocalCiRunner(root,mode='component',run_id='474-warnings-release-f2c4')
argv=['cargo','build','--release','-p','hex-desktop','--locked','--message-format=json']
started=time.monotonic()
with r.guarded_release(adopt=True) as report:
 report['build_argv']=argv
 artifact=None
 with (r.output_dir/'release-build.log').open('w') as log:
  proc=subprocess.Popen(argv,cwd=root,env=r.env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  print('RELEASE_PID',proc.pid,flush=True)
  for line in proc.stdout:
   log.write(line);log.flush()
   try:message=json.loads(line)
   except ValueError:continue
   if message.get('reason')=='compiler-artifact' and message.get('target',{}).get('name')=='hex-desktop' and message.get('executable'):
    artifact=pathlib.Path(message['executable'])
  report['build_exit_code']=proc.wait()
 if report['build_exit_code']!=0 or artifact is None:raise RuntimeError('release build failed or artifact absent: see raw log')
 report['artifact']=r.identify_release_artifact(artifact,argv)
 report['duration_seconds']=round(time.monotonic()-started,3)
 print('RELEASE_ARTIFACT',json.dumps(report['artifact']),flush=True)
print('GUARDED_RELEASE_PASS',r.summary_path,flush=True)
