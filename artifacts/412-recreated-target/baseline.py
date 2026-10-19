from pathlib import Path
import sys,json,hashlib
sys.path.insert(0,str(Path('scripts/tests').resolve()))
import test_launch_restart as f
x=f.ReleaseFixture();x.setUp()
try:
    artifact=x.target/'release/hex';artifact.parent.mkdir();artifact.write_bytes(b'owned optimized fixture')
    x.record.update(state='ready',artifact={'path':str(artifact),'sha256':hashlib.sha256(artifact.read_bytes()).hexdigest()},build_argv=['cargo','build','--release'])
    x.save();raw=x.owner.read_bytes();old=x.record['target_identity'];retained=x.target.with_name('retained-original-target');x.target.rename(retained);x.target.mkdir()
    current=x.runner._directory_identity(x.target)
    assert old!=current
    sentinel=x.target/'external-cache';sentinel.write_bytes(b'preserve')
    result={'old_identity':old,'current_identity':current,'same_workspace':x.record['workspace_owner']=={k:x.runner.git_identity[k] for k in ('workspace_root','git_common_dir')},'owner_sha256':hashlib.sha256(raw).hexdigest()}
    for name,call in [('restart_preflight',lambda:x.read()),('automatic_guard',lambda:x.runner.guarded_release(adopt=True,expected_owner_sha256=hashlib.sha256(raw).hexdigest()).__enter__())]:
        try:call()
        except (f.restart.RestartError,f.fixtures.local_ci.TargetCacheError) as e:result[name]={'type':type(e).__name__,'message':str(e)}
        else:raise AssertionError('existing implicit path unexpectedly adopted replacement')
    assert x.owner.read_bytes()==raw and sentinel.read_bytes()==b'preserve'
    assert (retained/'release/hex').read_bytes()==b'owned optimized fixture' and x.runner.target_lock is None
    assert 'cargo clean' not in x.f.call_log.read_text()
    result.update(owner_unchanged=True,retained_artifact_unchanged=True,no_cleanup=True,lock_released=True)
    print(json.dumps(result,indent=2))
finally:x.doCleanups()
