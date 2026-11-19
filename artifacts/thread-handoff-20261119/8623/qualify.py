import json, os, pathlib, re, signal, subprocess, threading, time
root = pathlib.Path.cwd()
source = subprocess.check_output(['git','rev-parse','HEAD'], text=True).strip()
base = subprocess.check_output(['git','merge-base','HEAD','origin/main'], text=True).strip()
archive = root/'artifacts'/'8623'/('qualification-'+source[:12])
archive.mkdir(parents=True, exist_ok=True)
checks = [
 ['cargo','test','-p','hex-project','-p','hex-storage','-p','hex-engine','--lib','--locked','--','fade_clock_tests::','unsupported_outer_fade_is_refused_before_preparation_cache_lookup','audio_transform::frame_fade_preview::tests::','schema::long_granular::tests::'],
 ['cargo','test','-p','hex-storage','--test','audio_edit','--test','musical_routing','--locked'],
 ['cargo','clippy','-p','hex-project','-p','hex-storage','-p','hex-engine','-p','hex-session','-p','hex-ui','-p','hex-desktop','-p','hex-cli','-p','hex-audio','-p','hex-export','-p','hex-media','-p','hex-stream','-p','hex-sandbox','--all-targets','--locked','--','-D','warnings'],
]

quick_path = root/'artifacts/local-ci/20261119T154008Z-1ea19913/summary.json'
quick = json.loads(quick_path.read_text())
if quick['commit'] != source or quick['exit_code'] or quick['stale'] or quick['dirty']:
    raise SystemExit('quick evidence does not match exact clean candidate')
summary = {'kind':'reviewed change-based check set; not runner-certified','base':base,'source':source,'checks':[], 'reused_exact_candidate_quick':str(quick_path), 'quick_exit_code':quick['exit_code'], 'omitted_extra_ui_test_support_clippy':'Reviewed unchanged feature consumers: no UI or conditional signature change, only two AudioClipDomain constructors (project capture/storage DTO), both default compiled. Default all-targets Clippy checks all twelve owner/consumer packages; no waiver for later query/binding changes.'}

test_total = 0.0
for index, cmd in enumerate(checks, 1):
    if subprocess.check_output(['git','status','--porcelain'], text=True).strip():
        raise SystemExit('source changed or dirty before check')
    log_path = archive/f'{index:02d}.log'
    print('running', ' '.join(cmd), 'log', log_path, flush=True)
    started = time.monotonic()
    test_started = None
    timer = None
    expired = []
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, start_new_session=True)
    def expire():
        expired.append(True)
        os.killpg(proc.pid, signal.SIGTERM)
    with log_path.open('w') as log:
        for line in proc.stdout:
            log.write(line)
            log.flush()
            if cmd[:2] == ['cargo', 'test'] and line.lstrip().startswith('Running ') and test_started is None:
                test_started = time.monotonic()
                timer = threading.Timer(max(0, 30 - test_total), expire)
                timer.start()
    code = proc.wait()
    test_wall = time.monotonic()-test_started if test_started is not None else None
    if timer:
        timer.cancel()
    if test_wall is not None:
        test_total += test_wall
        summary['combined_test_setup_execution_seconds'] = test_total
    text = log_path.read_text()
    entry = {'command':cmd,'exit_code':code,'wall_seconds':time.monotonic()-started,'test_setup_execution_seconds':test_wall,'test_budget_exceeded':bool(expired),'log':str(log_path),'harness_summaries':re.findall(r'test result:.*',text)}
    summary['checks'].append(entry)
    (archive/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print(json.dumps(entry), flush=True)
    if code or expired:
        raise SystemExit(code or 1)
if subprocess.check_output(['git','status','--porcelain'], text=True).strip() or subprocess.check_output(['git','rev-parse','HEAD'], text=True).strip()!=source:
    raise SystemExit('source changed during qualification')
summary['passed']=True
(archive/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print('PASSED',archive/'summary.json',flush=True)
