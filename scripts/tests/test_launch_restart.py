"""Opt-in lifecycle tests: only fixture-owned children and disposable targets."""
from dataclasses import asdict, replace
from contextlib import ExitStack
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import test_local_ci as fixtures
import launch_restart_support as support
restart, ci, delivery = support.restart, fixtures.local_ci, support.delivery


class OwnedProcess:
    """Fixture adapter never discovers or signals a process it did not create."""
    def __init__(self, app, *, ignore_quit=False):
        code = ('import signal,sys,time; '
                + ('signal.signal(signal.SIGTERM, signal.SIG_IGN); ' if ignore_quit else '')
                + 'print("ready",flush=True); time.sleep(60)')
        self.child = subprocess.Popen([sys.executable, '-c', code], stdout=subprocess.PIPE, text=True)
        assert self.child.stdout.readline().strip() == 'ready'
        self.identity = restart.Identity(self.child.pid, str(time.monotonic_ns()), str(app),
                                         str(app/'Contents/MacOS/hex'), 'org.portalsurfer.hex.dev', 1, 2, 1, 3)
        self.requests = []

    def snapshot(self):
        return [] if self.child.poll() is not None else [self.identity]

    def request(self, operation, expected):
        if expected != self.identity or self.child.poll() is not None:
            raise restart.RestartError('fixture identity changed')
        self.requests.append((operation, expected))
        self.child.send_signal(signal.SIGTERM if operation == 'quit' else signal.SIGKILL)
        return True

    def close(self):
        if self.child.poll() is None:
            self.child.kill()
        self.child.wait(timeout=5)
        self.child.stdout.close()


class LifecycleTests(unittest.TestCase):
    def child(self, **kwargs):
        p = OwnedProcess(Path('/fixture/hex.app'), **kwargs)
        self.addCleanup(p.close)
        return p

    def test_owned_normal_quit_and_timeout_force_leave_unrelated_child_alive(self):
        unrelated = self.child()
        for ignore, expected in ((False, ['quit']), (True, ['quit', 'force'])):
            with self.subTest(ignore=ignore):
                p = self.child(ignore_quit=ignore)
                restart.stop_exact(p, normal_timeout=.05, force_timeout=1, announce=lambda *a, **k: None)
                self.assertEqual([op for op, _ in p.requests], expected)
                self.assertIsNotNone(p.child.poll())
                self.assertIsNone(unrelated.child.poll())

    def test_absent_app_never_requests_quit_or_prints_close_guidance(self):
        c = Mock(); c.snapshot.return_value = []; announce = Mock()
        restart.stop_exact(c, announce=announce)
        c.request.assert_not_called(); announce.assert_not_called()

    def test_multiple_and_pid_replacement_never_retarget(self):
        p = self.child()
        c = Mock(); c.snapshot.return_value = [p.identity, p.identity]
        with self.assertRaises(restart.RestartError): restart.stop_exact(c)
        c.request.assert_not_called()
        for changed in (replace(p.identity, started='new-start'), replace(p.identity, pid=p.identity.pid+1)):
            c = Mock(); c.snapshot.side_effect = [[p.identity], [changed]]
            with self.assertRaisesRegex(restart.RestartError, 'identity changed'):
                restart.stop_exact(c, announce=lambda *a, **k: None)
            c.request.assert_not_called()

    def test_replacement_after_normal_quit_does_not_receive_force(self):
        expected=self.child().identity
        c=Mock();c.snapshot.side_effect=[[expected],[expected],[replace(expected,started='replacement')]]
        with self.assertRaisesRegex(restart.RestartError,'identity changed'):
            restart.stop_exact(c,announce=lambda *a,**k:None)
        self.assertEqual([x.args[0] for x in c.request.call_args_list],['quit'])

    def test_interrupt_during_normal_wait_never_escalates(self):
        p = self.child(ignore_quit=True)
        with self.assertRaises(KeyboardInterrupt):
            restart.stop_exact(p, sleep=Mock(side_effect=KeyboardInterrupt), announce=lambda *a, **k: None)
        self.assertEqual([op for op, _ in p.requests], ['quit'])
        self.assertIsNone(p.child.poll())

    def test_refused_native_request_and_force_exhaustion_are_bounded(self):
        identity = self.child().identity
        c = Mock(); c.snapshot.return_value = [identity]; c.request.return_value = False
        ticks = iter(range(20))
        with self.assertRaisesRegex(restart.RestartError, 'did not exit'):
            restart.stop_exact(c, normal_timeout=0, force_timeout=0, clock=lambda: next(ticks), announce=lambda *a, **k: None)
        self.assertEqual([x.args[0] for x in c.request.call_args_list], ['quit', 'force'])

    def test_exact_identity_rejects_nonhex_paths_boolean_pid_and_missing_start(self):
        app = Path('/fixture/hex.app'); good = asdict(self.child().identity)
        for key, value in [('pid', True), ('started', ''), ('bundle', '/other/hex.app'),
                           ('executable', '/other/hex'), ('identifier', 'org.other.hex')]:
            with self.subTest(key=key), self.assertRaises(restart.RestartError):
                restart.exact_identity({**good, key:value}, app)

    def test_native_disk_replacement_and_native_errors_never_send_action(self):
        with tempfile.TemporaryDirectory() as d:
            out=Path(d).resolve(); app=out/'hex.app'
            c=restart.NativeController(out,app,out,Mock()); c.seal=(1,2,'old')
            with patch.object(c,'disk_identity',return_value=(1,3,'new')), patch.object(restart.subprocess,'Popen') as run:
                with self.assertRaisesRegex(restart.RestartError,'changed'): c.call('force')
                run.assert_not_called()
            c=restart.NativeController(out,app,out,Mock()); c.seal=(1,2,'old')
            with patch.object(c,'disk_identity',return_value=c.seal), \
                    patch.object(c,'_prepare_helper',return_value=out/'fixture-helper'), \
                    patch.object(c,'_run_owned',side_effect=restart.RestartError('native quit timed out; escalation stopped')):
                with self.assertRaisesRegex(restart.RestartError,'timed out'): c.call('quit')



class FilesystemIdentityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.app = self.root / 'hex.app'
        self.executable = self.app / 'Contents/MacOS/hex'
        self.executable.parent.mkdir(parents=True)
        self.executable.write_bytes(b'owned fixture executable')
        self.delivery = Mock()
        self.delivery.sha.side_effect = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()

    def payload(self):
        bundle = self.app.lstat(); executable = self.executable.lstat()
        return dict(pid=123, started='fixture-start', bundle=str(self.app),
                    executable=str(self.executable), identifier='org.portalsurfer.hex.dev',
                    bundle_device=bundle.st_dev, bundle_inode=bundle.st_ino,
                    executable_device=executable.st_dev, executable_inode=executable.st_ino)

    def test_actual_target_pins_and_observed_strings_are_preserved(self):
        value = self.payload()
        self.assertEqual(asdict(restart.exact_identity(value, self.app)), value)

    def test_alternate_spelling_requires_same_filesystem_pins_model(self):
        # Model ONLY alternate-spelling lstat lookup using real owned pins.
        # The unmocked oracle below follows this fixture filesystem's actual
        # identity relationship; the model is not native-volume evidence.
        value = self.payload(); alias = self.root / 'Hex.app'
        value.update(bundle=str(alias), executable=str(alias/'Contents/MacOS/hex'))
        original = restart._path_pin
        def case_insensitive_pin(path, **kwargs):
            relative = path.relative_to(alias)
            return original(self.app / relative, **kwargs)
        def lookup(path, **kwargs):
            return case_insensitive_pin(path, **kwargs) if path == alias or alias in path.parents else original(path, **kwargs)
        with patch.object(restart, '_path_pin', side_effect=lookup):
            self.assertEqual(asdict(restart.exact_identity(value, self.app)), value)
        if alias.exists():
            self.assertTrue(alias.samefile(self.app))
            self.assertTrue((alias/'Contents/MacOS/hex').samefile(self.executable))
            self.assertEqual(asdict(restart.exact_identity(value, self.app)), value)
        else:
            with self.assertRaises(restart.RestartError):
                restart.exact_identity(value, self.app)

    def test_distinct_case_sensitive_bundle_and_hardlinked_executable_refuse(self):
        def assert_distinct_bundle_refuses(other):
            executable = other/'Contents/MacOS/hex'
            executable.parent.mkdir(parents=True)
            os.link(self.executable, executable)
            value = self.payload()
            value.update(bundle=str(other), executable=str(executable))
            self.assertFalse(other.samefile(self.app))
            self.assertTrue(executable.samefile(self.executable))
            with self.assertRaisesRegex(restart.RestartError, 'another target'):
                restart.exact_identity(value, self.app)
            # Honest pins for the distinct bundle still cannot authorize ours.
            bundle = other.stat()
            value.update(bundle_device=bundle.st_dev, bundle_inode=bundle.st_ino)
            with self.assertRaisesRegex(restart.RestartError, 'another target'):
                restart.exact_identity(value, self.app)

        # A different parent guarantees a distinct actual bundle on either
        # filesystem, without changing the supported Hex.app basename.
        assert_distinct_bundle_refuses(self.root/'independent'/'Hex.app')
        other = self.root/'Hex.app'
        if other.exists():
            self.assertTrue(other.samefile(self.app))
        else:
            # Retain the real case-only negative wherever it is representable.
            assert_distinct_bundle_refuses(other)

    def test_pin_types_bounds_zero_inode_and_missing_extra_fields_refuse(self):
        good = self.payload()
        for field in ('bundle_device','bundle_inode','executable_device','executable_inode'):
            for invalid in (True, False, -1, 1 << 64, '1', 1.0, None):
                with self.subTest(field=field,value=invalid), self.assertRaises(restart.RestartError):
                    restart.exact_identity({**good, field:invalid}, self.app)
            missing = dict(good); del missing[field]
            with self.assertRaises(restart.RestartError): restart.exact_identity(missing,self.app)
        for field in ('bundle_inode','executable_inode'):
            with self.assertRaises(restart.RestartError): restart.exact_identity({**good,field:0},self.app)
        with self.assertRaises(restart.RestartError): restart.exact_identity({**good,'extra':1},self.app)
        for pid in (True, 0, -1, 1 << 31):
            with self.assertRaises(restart.RestartError): restart.exact_identity({**good,'pid':pid},self.app)

    def test_zero_device_and_uint64_limit_are_representable_protocol_values(self):
        # Boundary model; not a claim about this host's actual device/inode IDs.
        for pins in ((0,1,0,2), ((1 << 64)-1,)*4):
            value = self.payload()
            value.update(zip(('bundle_device','bundle_inode','executable_device','executable_inode'),pins))
            with patch.object(restart,'_filesystem_identity',return_value=pins):
                self.assertEqual(asdict(restart.exact_identity(value,self.app)),value)

    def test_symlink_bundle_parent_executable_and_traversed_dotdot_refuse(self):
        good = self.payload(); alias = self.root/'alias'; alias.symlink_to(self.root, target_is_directory=True)
        for bundle, executable in ((alias/'hex.app',alias/'hex.app/Contents/MacOS/hex'),
                                   (self.app,alias/'hex.app/Contents/MacOS/hex'),
                                   (alias/'..'/self.root.name/'hex.app', self.executable)):
            value = {**good,'bundle':str(bundle),'executable':str(executable)}
            with self.subTest(bundle=bundle), self.assertRaises(restart.RestartError):
                restart.exact_identity(value,self.app)
        saved=self.root/'saved'; self.executable.rename(saved); self.executable.symlink_to(saved)
        with self.assertRaisesRegex(restart.RestartError,'symlink'): restart.exact_identity(good,self.app)

    def test_bundle_and_executable_inode_replacements_refuse_old_snapshot(self):
        good=self.payload()
        saved=self.root/'old-executable'; self.executable.rename(saved)
        self.executable.write_bytes(saved.read_bytes())
        with self.assertRaisesRegex(restart.RestartError,'identity changed'):
            restart.exact_identity(good,self.app)
        self.executable.unlink(); saved.rename(self.executable)
        old=self.root/'old-bundle'; self.app.rename(old)
        self.executable.parent.mkdir(parents=True)
        os.link(old/'Contents/MacOS/hex', self.executable)
        with self.assertRaisesRegex(restart.RestartError,'identity changed'):
            restart.exact_identity(good,self.app)

    def test_invalid_process_strings_and_nonregular_targets_refuse(self):
        good=self.payload()
        for key,value in (('started',''),('started',None),('identifier','other.app'),
                          ('bundle','relative/hex.app'),('executable',None)):
            with self.subTest(key=key,value=value),self.assertRaisesRegex(restart.RestartError,'ambiguous'):
                restart.exact_identity({**good,key:value},self.app)
        saved=self.root/'saved';self.executable.rename(saved);self.executable.mkdir()
        with self.assertRaisesRegex(restart.RestartError,'ambiguous'):restart.exact_identity(good,self.app)
        self.executable.rmdir();saved.rename(self.executable)
        self.delivery.verify.side_effect=RuntimeError('foreign signature')
        with self.assertRaisesRegex(RuntimeError,'foreign signature'):
            restart.NativeController(self.root,self.app,self.root,self.delivery)

    def test_target_change_between_observed_and_final_read_refuses(self):
        good=self.payload(); pins=tuple(good[k] for k in ('bundle_device','bundle_inode','executable_device','executable_inode'))
        changed=(pins[0],pins[1]+1,*pins[2:])
        with patch.object(restart,'_filesystem_identity',side_effect=[pins,pins,changed]):
            with self.assertRaisesRegex(restart.RestartError,'identity changed'):
                restart.exact_identity(good,self.app)

    def test_installed_seal_tracks_bundle_inode_even_if_executable_is_same_hardlink(self):
        controller=restart.NativeController(self.root,self.app,self.root,self.delivery)
        seal=controller.seal
        old=self.root/'old-bundle';self.app.rename(old)
        self.executable.parent.mkdir(parents=True);os.link(old/'Contents/MacOS/hex',self.executable)
        self.assertNotEqual(controller.disk_identity(),seal)
        self.assertEqual(controller.disk_identity()[2:],seal[2:])
        with patch.object(controller,'_prepare_helper') as compile:
            with self.assertRaisesRegex(restart.RestartError,'changed'): controller.call('force')
            compile.assert_not_called()

    def test_changed_request_identity_refuses_before_compile_and_after_compile(self):
        good=restart.exact_identity(self.payload(),self.app)
        for during_compile in (False,True):
            controller=restart.NativeController(self.root,self.app,self.root,self.delivery)
            wrong=replace(good,bundle_inode=good.bundle_inode+1)
            with patch.object(controller,'_prepare_helper') as compile, patch.object(controller,'_run_owned') as operation:
                if during_compile:
                    # Same disk seal check is not enough if native identity pins
                    # change; exercise the second actual protocol validation.
                    original=restart.exact_identity
                    count=[0]
                    def validate(value,app):
                        count[0]+=1
                        if count[0]==2: value={**value,'bundle_inode':wrong.bundle_inode}
                        return original(value,app)
                    with patch.object(restart,'exact_identity',side_effect=validate):
                        with self.assertRaises(restart.RestartError): controller.call('quit',good)
                    compile.assert_called_once()
                else:
                    with self.assertRaises(restart.RestartError): controller.call('quit',wrong)
                    compile.assert_not_called()
                operation.assert_not_called()
                with self.assertRaisesRegex(restart.RestartError,'closed'):
                    controller._prepare_helper = restart.NativeController._prepare_helper.__get__(controller)
                    controller._prepare_helper()


class FixturePathTests(unittest.TestCase):
    def test_aliased_temporary_parent_reuses_the_actual_owner_marker(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory).resolve()
            physical = parent / 'physical'
            physical.mkdir()
            alias = parent / 'alias'
            alias.symlink_to(physical, target_is_directory=True)
            owned = tempfile.TemporaryDirectory(dir=alias)
            fixture = fixtures.LocalCiRunnerTests()
            try:
                with patch.object(fixtures.tempfile, 'TemporaryDirectory', return_value=owned):
                    fixture.setUp()
                target = Path(fixture.env['FAKE_METADATA_TARGET'])
                target.mkdir()
                first = fixture.runner('aliased-parent-first')
                self.assertEqual(first.run(), 0, first.summary)
                _, owner = fixtures.local_ci.target_state_paths(target, fixture.target_state_dir)
                # The fixture's public target spelling must find the real record.
                self.assertEqual(json.loads(owner.read_bytes())['target_directory'], str(target))
                retained = target / 'external-cache'
                retained.write_bytes(b'preserve')
                second = fixture.runner('aliased-parent-reuse')
                self.assertEqual(second.run(), 0, second.summary)
                self.assertEqual(second.summary['target_cache']['cleanup_status'], 'not_needed')
                self.assertEqual(retained.read_bytes(), b'preserve')
            finally:
                fixture.doCleanups()
                owned.cleanup()


class ReleaseFixture(unittest.TestCase):
    def setUp(self):
        self.f = fixtures.LocalCiRunnerTests(); self.f.setUp(); self.addCleanup(self.f.tearDown)
        fixtures.install_delivery_source_fixture(self.f.repo)
        self.runner = self.f.runner('restart-preflight')
        self.assertTrue(self.runner._capture_repository_state('start_tree'))
        self.assertTrue(self.runner._resolve_cargo_identity())
        self.target=Path(self.runner.cargo_identity['target_directory']); self.target.mkdir()
        _, dev = ci.target_state_paths(self.target, self.runner.target_state_dir)
        self.owner=dev.with_name(dev.name.replace('.owner.json','.release-owner.json'));self.owner.parent.mkdir(exist_ok=True)
        self.source='a'*40
        self.record={'schema_version':1,'profile':'release','state':'pending',
                     'target_directory':str(self.target),'target_identity':self.runner._directory_identity(self.target),
                     'workspace_owner':{k:self.runner.git_identity[k] for k in ('workspace_root','git_common_dir')},
                     'source':{'commit':self.source,'dirty':False,'dirty_files':[],'content_sha256':'b'*64},
                     'packages':self.runner._release_scope()}
        self.save()

    def save(self): self.owner.write_text(json.dumps(self.record))
    def read(self): return restart.recovery_hash(self.runner,fixtures.local_ci,self.source,lambda old,new: old==new)



class OwnershipTests(ReleaseFixture):
    def test_pending_is_read_only_and_ready_is_verified_without_adoption(self):
        raw=self.owner.read_bytes(); self.assertEqual(self.read(),hashlib.sha256(raw).hexdigest());self.assertEqual(raw,self.owner.read_bytes())
        artifact=self.target/'release/hex';artifact.parent.mkdir();artifact.write_bytes(b'fixture')
        self.record.update(state='ready',artifact={'path':str(artifact),'sha256':hashlib.sha256(artifact.read_bytes()).hexdigest()},build_argv=['cargo','build','--release'])
        self.save();self.runner.target_lock=object()
        self.assertIsNone(self.read())

    def test_unknown_foreign_source_package_target_and_malformed_refuse_preserving_bytes(self):
        import copy
        original=copy.deepcopy(self.record)
        mutations=[lambda r:r.update(state='unknown'),lambda r:r['workspace_owner'].update(workspace_root='/foreign'),
                   lambda r:r['target_identity'].update(inode=0),lambda r:r['source'].update(commit='c'*40),
                   lambda r:r['packages'][0].update(spec='foreign'),lambda r:r.update(packages=[])]
        for change in mutations:
            self.record=copy.deepcopy(original);change(self.record);self.save();raw=self.owner.read_bytes()
            with self.assertRaises(restart.RestartError):self.read()
            self.assertEqual(raw,self.owner.read_bytes())
        for raw in (b'null',b'[]',b'{bad'):
            self.owner.write_bytes(raw)
            with self.assertRaises(restart.RestartError):self.read()
            self.assertEqual(raw,self.owner.read_bytes())
        self.owner.unlink();(self.target/'foreign').write_bytes(b'keep')
        with self.assertRaisesRegex(restart.RestartError,'unknown'):self.read()
        (self.target/'foreign').unlink();self.assertIsNone(self.read())

    def test_symlink_owner_and_oversized_owner_never_read_or_modify_foreign_data(self):
        raw=self.owner.read_bytes();foreign=self.f.base/'foreign';foreign.write_bytes(raw);self.owner.unlink();self.owner.symlink_to(foreign)
        with self.assertRaises(restart.RestartError):self.read()
        self.assertEqual(foreign.read_bytes(),raw);self.owner.unlink();self.owner.write_bytes(b'x'*(128*1024+1))
        with self.assertRaisesRegex(restart.RestartError,'bounded'):self.read()

    def test_ready_symlink_artifact_is_refused_like_existing_release_guard(self):
        artifact=self.target/'release/hex';artifact.parent.mkdir();artifact.write_bytes(b'fixture')
        alias=artifact.with_name('alias');alias.symlink_to(artifact)
        self.record.update(state='ready',artifact={'path':str(alias),'sha256':hashlib.sha256(artifact.read_bytes()).hexdigest()},build_argv=['cargo','build','--release'])
        self.save();self.runner.target_lock=object()
        with self.assertRaises(restart.RestartError):self.read()
        self.assertTrue(alias.is_symlink());self.assertEqual(artifact.read_bytes(),b'fixture')

    def test_owner_cas_inside_actual_mutation_lock_refuses_changed_bytes_before_cleanup(self):
        raw=self.owner.read_bytes();expected=hashlib.sha256(raw).hexdigest()
        acquire=fixtures.local_ci.CargoTargetLock.acquire
        def acquire_then_replace(lock):
            acquire(lock)
            self.owner.write_bytes(raw+b'\n')
        with patch.object(fixtures.local_ci.CargoTargetLock,'acquire',acquire_then_replace):
            with self.assertRaisesRegex(fixtures.local_ci.TargetCacheError,'changed since'):
                with self.runner.guarded_release(adopt=True,expected_owner_sha256=expected):self.fail('entered mutation')
        self.assertEqual(self.owner.read_bytes(),raw+b'\n');self.assertIsNone(self.runner.target_lock)
        self.assertFalse(self.runner.summary.get('release_target_cache'))
        self.assertNotIn('cargo clean',self.f.call_log.read_text())

    def assert_recovery_refuses_without_mutation(self, expected, raw):
        sentinel = self.f.base / 'unrelated-directory' / 'keep'
        sentinel.parent.mkdir(exist_ok=True)
        sentinel.write_bytes(b'unrelated data')
        calls = self.f.call_log.read_text()
        with self.assertRaisesRegex(fixtures.local_ci.TargetCacheError, 'identity|workspace'):
            with self.runner.guarded_release(adopt=True, expected_owner_sha256=expected):
                self.fail('automatic recovery entered mutation after identity changed')
        self.assertEqual(self.owner.read_bytes(), raw)
        self.assertEqual(sentinel.read_bytes(), b'unrelated data')
        self.assertIsNone(self.runner.target_lock)
        self.assertFalse(self.runner.summary.get('release_target_cache'))
        self.assertNotIn('cargo clean', self.f.call_log.read_text()[len(calls):])

    def test_owner_cas_refuses_replaced_target_with_unchanged_marker(self):
        expected = self.read()  # Actual read-only automatic restart preflight.
        raw = self.owner.read_bytes()
        self.target.rename(self.f.repo / 'retained-target')
        self.target.mkdir()
        keep = self.target / 'foreign-cache'
        keep.write_bytes(b'not the inspected target')
        self.assert_recovery_refuses_without_mutation(expected, raw)
        self.assertEqual(keep.read_bytes(), b'not the inspected target')

    def test_owner_cas_refuses_missing_target_with_unchanged_marker(self):
        expected = self.read()
        raw = self.owner.read_bytes()
        self.target.rename(self.f.repo / 'retained-target')
        self.assert_recovery_refuses_without_mutation(expected, raw)
        self.assertFalse(self.target.exists())

    def test_owner_cas_refuses_foreign_workspace_even_with_matching_marker_hash(self):
        for field in ('workspace_root', 'git_common_dir'):
            with self.subTest(field=field):
                previous = self.record['workspace_owner'][field]
                self.record['workspace_owner'][field] = str(self.f.base / 'foreign')
                self.save()
                raw = self.owner.read_bytes()
                self.assert_recovery_refuses_without_mutation(hashlib.sha256(raw).hexdigest(), raw)
                self.record['workspace_owner'][field] = previous

    def test_matching_owner_cas_uses_existing_release_rebuild_and_commits_ready(self):
        expected=hashlib.sha256(self.owner.read_bytes()).hexdigest()
        keep=self.target/'external-cache';keep.write_bytes(b'external')
        with self.runner.guarded_release(adopt=True,expected_owner_sha256=expected) as report:
            artifact=self.target/'release/fixture';artifact.parent.mkdir(exist_ok=True);artifact.write_bytes(b'new optimized')
            argv=['cargo','build','--release']
            report.update(build_exit_code=0,build_argv=argv,artifact=self.runner.identify_release_artifact(artifact,argv))
            self.assertEqual(report['cleanup_argv'][3],'release')
        self.assertEqual(json.loads(self.owner.read_text())['state'],'ready')
        self.assertEqual(keep.read_bytes(),b'external');self.assertIsNone(self.runner.target_lock)


class RecreatedTargetTests(ReleaseFixture):
    """Explicit authority is distinct from automatic restart and manual adoption."""
    def setUp(self):
        super().setUp()
        tool = self.f.bin / 'git'
        tool.write_text(tool.read_text().replace('deadbeef', self.source))
        self.assertTrue(self.runner._capture_repository_state('start_tree'))
        self.record.update(state='ready', artifact={'path':str(self.target/'release/hex'), 'sha256':'d'*64},
                           build_argv=['cargo','build','--release'])
        self.save()
        self.retained = self.target.with_name('retained-original-target')
        (self.target/'old-artifact').write_bytes(b'never qualify or erase old build')
        self.target.rename(self.retained)
        self.target.mkdir()
        self.keep = self.target/'external-cache'
        self.keep.write_bytes(b'preserve external data')

    def inspect(self):
        path, _ = fixtures.local_ci.target_state_paths(self.target, self.runner.target_state_dir)
        lock = fixtures.local_ci.CargoTargetLock(path, {'operation':'fixture-inspection'})
        lock.acquire(); self.runner.target_lock = lock
        try:
            return self.runner.inspect_recreated_release_target()
        finally:
            self.assertIsNone(lock.release()); self.runner.target_lock = None

    def guard(self, authority, check=lambda: None):
        return self.runner.guarded_release(adopt=True, expected_owner_sha256=authority['owner_sha256'],
            expected_recreated_target_identity=authority['target_identity'], recovery_inactive_check=check)

    def assert_preserved(self, raw, calls):
        self.assertEqual(self.owner.read_bytes(), raw)
        self.assertEqual(self.keep.read_bytes(), b'preserve external data')
        self.assertEqual((self.retained/'old-artifact').read_bytes(), b'never qualify or erase old build')
        self.assertNotIn('cargo clean', self.f.call_log.read_text()[len(calls):])
        self.assertIsNone(self.runner.target_lock)

    def test_explicit_inspection_and_real_guard_rebuild_ready_without_stale_artifact_reuse(self):
        raw = self.owner.read_bytes(); authority = self.inspect()
        self.assertEqual(authority['owner_sha256'], hashlib.sha256(raw).hexdigest())
        self.assertEqual(authority['target_identity'], self.runner._directory_identity(self.target))
        self.assertNotEqual(authority['target_identity'], self.record['target_identity'])
        self.assertEqual(self.owner.read_bytes(), raw)
        inactive = Mock()
        with self.guard(authority, inactive) as report:
            self.assertEqual(report['state'],'pending')
            self.assertEqual(report['previous_owner'], self.record)
            self.assertEqual(report['cleanup_argv'][3],'release')
            artifact = self.target/'release/fixture'; artifact.parent.mkdir(); artifact.write_bytes(b'fresh fixture')
            argv = ['cargo','build','--release']
            report.update(build_argv=argv,build_exit_code=0,
                          artifact=self.runner.identify_release_artifact(artifact,argv))
        inactive.assert_called_once_with()
        ready=json.loads(self.owner.read_bytes())
        self.assertEqual(ready['state'],'ready')
        self.assertEqual(ready['target_identity'],authority['target_identity'])
        self.assertEqual(ready['artifact']['sha256'],hashlib.sha256(b'fresh fixture').hexdigest())
        self.assertEqual(self.keep.read_bytes(),b'preserve external data')
        self.assertEqual((self.retained/'old-artifact').read_bytes(),b'never qualify or erase old build')
        self.assertIsNone(self.runner.target_lock)

    def test_default_and_force_owner_cas_still_refuse_recreated_target(self):
        raw=self.owner.read_bytes();calls=self.f.call_log.read_text()
        with self.assertRaises(restart.RestartError):self.read()
        for kwargs in ({}, {'adopt':True,'expected_owner_sha256':hashlib.sha256(raw).hexdigest()}):
            with self.assertRaises(fixtures.local_ci.TargetCacheError):
                with self.runner.guarded_release(**kwargs):self.fail('implicit adoption')
        self.assert_preserved(raw,calls)

    def test_changed_or_missing_replacement_after_inspection_refuses_before_mutation(self):
        authority=self.inspect();raw=self.owner.read_bytes();calls=self.f.call_log.read_text()
        saved=self.target.with_name('inspected-replacement');self.target.rename(saved)
        for exists in (False,True):
            if exists:self.target.mkdir()
            with self.assertRaises(fixtures.local_ci.TargetCacheError):
                with self.guard(authority):self.fail('replacement race accepted')
            self.assertEqual(self.owner.read_bytes(),raw)
            self.assertNotIn('cargo clean',self.f.call_log.read_text()[len(calls):])
            self.assertIsNone(self.runner.target_lock)
        self.target.rmdir();saved.rename(self.target)
        self.assert_preserved(raw,calls)

    def test_owner_cas_change_after_inspection_refuses_before_mutation(self):
        authority=self.inspect();raw=self.owner.read_bytes()+b'\n';self.owner.write_bytes(raw)
        calls=self.f.call_log.read_text()
        with self.assertRaises(fixtures.local_ci.TargetCacheError):
            with self.guard(authority):self.fail('owner race accepted')
        self.assert_preserved(raw,calls)

    def test_foreign_source_packages_and_malformed_historical_owner_refuse(self):
        import copy
        original=copy.deepcopy(self.record)
        mutations=[lambda r:r['workspace_owner'].update(workspace_root='/foreign'),
                   lambda r:r['workspace_owner'].update(git_common_dir='/foreign'),
                   lambda r:r['packages'][0].update(spec='foreign'),
                   lambda r:r['packages'][0].update(version='other'),
                   lambda r:r['packages'][0].update(name='other'),
                   lambda r:r.update(packages=[]), lambda r:r.update(state='foreign'),
                   lambda r:r['source'].update(dirty=True), lambda r:r['source'].update(commit='bad'),
                   lambda r:r['source'].update(content_sha256='bad'),
                   lambda r:r.update(target_identity={'device':True,'inode':1}),
                   lambda r:r.update(target_identity={'device':1,'inode':0}),
                   lambda r:r.update(target_identity=self.runner._directory_identity(self.target)),
                   lambda r:r['artifact'].update(path='/outside/hex'),
                   lambda r:r['artifact'].update(sha256='bad'),lambda r:r.update(build_argv='cargo')]
        for change in mutations:
            with self.subTest(change=mutations.index(change)):
                self.record=copy.deepcopy(original);change(self.record);self.save()
                raw=self.owner.read_bytes();calls=self.f.call_log.read_text()
                with self.assertRaises(fixtures.local_ci.TargetCacheError):self.inspect()
                self.assert_preserved(raw,calls)
        for raw in (b'null',b'[]',b'{bad',b'x'*(128*1024+1)):
            self.owner.write_bytes(raw);calls=self.f.call_log.read_text()
            with self.assertRaises(fixtures.local_ci.TargetCacheError):self.inspect()
            self.assert_preserved(raw,calls)

    def test_nonancestor_source_refuses_and_never_cleans(self):
        raw=self.owner.read_bytes();calls=self.f.call_log.read_text();original=fixtures.local_ci.subprocess.run
        def command(argv,**kwargs):
            if argv[:3]==['git','merge-base','--is-ancestor']:
                return subprocess.CompletedProcess(argv,1)
            return original(argv,**kwargs)
        with patch.object(fixtures.local_ci.subprocess,'run',side_effect=command):
            with self.assertRaisesRegex(fixtures.local_ci.TargetCacheError,'ancestor'):self.inspect()
        self.assert_preserved(raw,calls)

    def test_symlink_owner_or_target_and_held_lock_refuse(self):
        raw=self.owner.read_bytes();calls=self.f.call_log.read_text()
        foreign=self.f.base/'foreign-marker';foreign.write_bytes(raw);self.owner.unlink();self.owner.symlink_to(foreign)
        with self.assertRaises(fixtures.local_ci.TargetCacheError):self.inspect()
        self.assertEqual(foreign.read_bytes(),raw);self.owner.unlink();self.owner.write_bytes(raw)
        moved=self.target.with_name('plain-replacement');self.target.rename(moved);self.target.symlink_to(moved,target_is_directory=True)
        with self.assertRaisesRegex(fixtures.local_ci.TargetCacheError,'symlink'):self.inspect()
        self.target.unlink();moved.rename(self.target)
        authority=self.inspect();path,_=fixtures.local_ci.target_state_paths(self.target,self.runner.target_state_dir)
        lock=fixtures.local_ci.CargoTargetLock(path,{'operation':'other-job'});lock.acquire()
        try:
            with self.assertRaisesRegex(fixtures.local_ci.TargetCacheError,'already in use'):
                with self.guard(authority):self.fail('live job bypassed')
        finally:self.assertIsNone(lock.release())
        self.assert_preserved(raw,calls)

    def test_lexical_parent_alias_refuses_even_with_the_same_physical_target(self):
        raw = self.owner.read_bytes()
        calls = self.f.call_log.read_text()
        self.inspect()  # The ordinary, physical fixture path is eligible.
        alias = self.f.base / 'aliased-parent'
        alias.symlink_to(self.f.repo, target_is_directory=True)
        lexical = alias / self.target.relative_to(self.f.repo)
        self.runner.env['FAKE_METADATA_TARGET'] = str(lexical)
        self.runner.env['CARGO_TARGET_DIR'] = str(lexical)
        self.assertTrue(self.runner._resolve_cargo_identity())
        self.assertEqual(self.runner.cargo_identity['target_directory'], str(self.target))
        with self.assertRaisesRegex(fixtures.local_ci.TargetCacheError, 'symlink'):
            self.inspect()
        self.assert_preserved(raw, calls)

    def test_last_boundary_rechecks_inactivity_owner_target_and_source(self):
        for cause in ('running','marker','target','source'):
            with self.subTest(cause=cause):
                authority=self.inspect();raw=self.owner.read_bytes();calls=self.f.call_log.read_text()
                def check():
                    if cause=='running':raise delivery.DeliveryError('running')
                    if cause=='marker':self.owner.write_bytes(raw+b'\n')
                    if cause=='target':self.target.rename(self.target.with_name('raced-target'));self.target.mkdir()
                    if cause=='source':self.runner.env['FAKE_FILE_LIST']='new-source'
                with self.assertRaises((fixtures.local_ci.TargetCacheError,delivery.DeliveryError)):
                    with self.guard(authority,check):self.fail('boundary change accepted')
                self.assertNotIn('cargo clean',self.f.call_log.read_text()[len(calls):])
                self.assertIsNone(self.runner.target_lock)
                self.assertEqual(self.owner.read_bytes(),raw+(b'\n' if cause=='marker' else b''))
                self.owner.write_bytes(raw)
                if cause=='target':self.target.rmdir();self.target.with_name('raced-target').rename(self.target)
                self.runner.env['FAKE_FILE_LIST']=''

    def test_missing_authority_or_inactivity_callback_cannot_relax_automatic_guard(self):
        authority=self.inspect();raw=self.owner.read_bytes();calls=self.f.call_log.read_text()
        for replacement in (True,{}, {'device':0,'inode':True},authority['target_identity']):
            with self.assertRaises(fixtures.local_ci.TargetCacheError):
                with self.runner.guarded_release(adopt=True,expected_owner_sha256=authority['owner_sha256'],
                    expected_recreated_target_identity=replacement):self.fail('incomplete authority accepted')
        self.assert_preserved(raw,calls)

    def test_explicit_launcher_inspection_never_constructs_controller_or_mutates_owner(self):
        app=self.f.repo.resolve()/'artifacts/hex.app';raw=self.owner.read_bytes();check=Mock()
        with patch.object(fixtures.local_ci,'LocalCiRunner',return_value=self.runner), \
                patch.object(delivery,'assert_inactive') as inactive, \
                patch.object(restart,'NativeController',side_effect=AssertionError('must not control app')):
            authority=restart.inspect_recreated_target(self.f.repo,app,self.source,fixtures.local_ci,delivery,check)
        self.assertEqual(authority['owner_sha256'],hashlib.sha256(raw).hexdigest())
        self.assertEqual(self.owner.read_bytes(),raw);self.assertEqual(inactive.call_count,2)
        self.assertEqual(check.call_count,2);self.assertIsNone(self.runner.target_lock)
        reports=list((self.f.repo/'artifacts/launch').glob('target-recovery-*/target-recovery-inspection.json'))
        self.assertEqual(json.loads(reports[0].read_text())['status'],'inspected_only')

    def test_inspection_raw_or_app_running_never_controls_app_or_mutates_owner(self):
        app=self.f.repo.resolve()/'artifacts/hex.app';raw=self.owner.read_bytes()
        for app_running in (True,False):
            with patch.object(fixtures.local_ci,'LocalCiRunner',return_value=self.runner), \
                    patch.object(delivery,'assert_inactive',side_effect=delivery.DeliveryError('running') if app_running else None), \
                    patch.object(restart,'NativeController') as controller:
                with self.assertRaises(delivery.DeliveryError):
                    restart.inspect_recreated_target(self.f.repo,app,self.source,fixtures.local_ci,delivery,
                        Mock(side_effect=None if app_running else delivery.DeliveryError('raw running')))
            controller.assert_not_called();self.assertEqual(self.owner.read_bytes(),raw)
            self.assertIsNone(self.runner.target_lock)
    def install_fixture_cargo(self):
        cargo=self.f.bin/'cargo';original=cargo.read_text()
        # The owned executable emits a real Cargo-shaped artifact record and
        # materializes that fixture file; metadata/clean stay the original fake.
        prefix=('#!'+sys.executable+'\nimport os,sys,json\nfrom pathlib import Path\n'
                'if sys.argv[1:2]==["build"]:\n'
                ' p=Path(os.environ["CARGO_TARGET_DIR"])/"release/fixture"\n'
                ' p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(b"fresh packaged fixture")\n'
                ' print(json.dumps({"reason":"compiler-artifact","executable":str(p),'
                '"target":{"name":"hex-desktop"},"profile":{"opt_level":"3"}}));sys.exit(0)\n'
                'os.execv("/bin/sh",["/bin/sh",str(Path(__file__).with_name("cargo-original")),*sys.argv[1:]])\n')
        (cargo.with_name('cargo-original')).write_text(original.replace('tool=${0##*/}', 'tool=cargo'));cargo.write_text(prefix)

    def test_packaging_refuses_recreated_target_without_release_cache_proof(self):
        authority = self.inspect()
        inactive = Mock()
        raw, calls = self.owner.read_bytes(), self.f.call_log.read_text()
        with self.assertRaisesRegex(fixtures.local_ci.TargetCacheError, 'release cache does not prove an empty origin'):
            with delivery.optimized_artifact(self.runner, adopt=True,
                    expected_owner_sha256=authority['owner_sha256'],
                    expected_recreated_target_identity=authority['target_identity'],
                    recovery_inactive_check=inactive):
                self.fail('unproven recreated target reached packaging')
        inactive.assert_not_called()
        self.assertNotIn('cargo build', self.f.call_log.read_text()[len(calls):])
        self.assert_preserved(raw, calls)

    def test_missing_plain_configured_app_parent_is_created_before_inspection_lock(self):
        app=self.f.repo.resolve()/'new-output'/'nested'/'hex.app'
        self.assertFalse(app.parent.exists());raw=self.owner.read_bytes()
        with patch.object(fixtures.local_ci,'LocalCiRunner',return_value=self.runner), \
                patch.object(delivery,'assert_inactive'),patch.object(restart,'NativeController') as controller:
            authority=restart.inspect_recreated_target(self.f.repo,app,self.source,fixtures.local_ci,delivery,Mock())
        self.assertTrue(app.parent.is_dir());self.assertFalse(app.exists())
        self.assertEqual(authority['owner_sha256'],hashlib.sha256(raw).hexdigest())
        self.assertEqual(self.owner.read_bytes(),raw);controller.assert_not_called()
        self.assertIsNone(self.runner.target_lock)

    def test_packaging_uses_fresh_target_after_unproven_pending_owner_refusal(self):
        authority=self.inspect();first_raw=self.owner.read_bytes()
        with self.assertRaisesRegex(RuntimeError,'fixture consumer failed'):
            with self.guard(authority):raise RuntimeError('fixture consumer failed')
        pending=json.loads(self.owner.read_bytes())
        self.assertEqual(pending['state'],'pending')
        self.assertEqual(pending['target_identity'],authority['target_identity'])
        self.assertNotEqual(self.owner.read_bytes(),first_raw)
        self.assertIsNone(self.runner.target_lock)
        # Exact pending ownership still lacks the guarded release cache proof.
        retry = self.inspect()
        raw, calls = self.owner.read_bytes(), self.f.call_log.read_text()
        self.assertEqual(retry['owner_sha256'], hashlib.sha256(raw).hexdigest())
        with self.assertRaisesRegex(fixtures.local_ci.TargetCacheError, 'release cache does not prove an empty origin'):
            with delivery.optimized_artifact(self.runner, adopt=True,
                    expected_owner_sha256=retry['owner_sha256'],
                    expected_recreated_target_identity=retry['target_identity'], recovery_inactive_check=Mock()):
                self.fail('unproven pending target reached packaging')
        self.assertNotIn('cargo build', self.f.call_log.read_text()[len(calls):])
        self.assert_preserved(raw, calls)

        self.install_fixture_cargo()
        fresh = self.target.with_name('fresh-release-target')
        fresh.mkdir()
        runner = self.f.runner('fresh-release', env={**self.runner.env, 'CARGO_TARGET_DIR': str(fresh),
                                                      'FAKE_METADATA_TARGET': str(fresh)})
        with delivery.optimized_artifact(runner) as (binary, report):
            self.assertEqual(binary.read_bytes(), b'fresh packaged fixture')
            self.assertEqual(report['build_cache_origin'], 'empty-target-v1')
        self.assertEqual(json.loads(Path(report['owner_path']).read_bytes())['state'], 'ready')
        self.assert_preserved(raw, calls)

    def test_missing_parent_refuses_symlink_and_nondirectory_without_marker_changes(self):
        raw=self.owner.read_bytes();foreign=self.f.base/'foreign-parent';foreign.mkdir()
        alias=self.f.repo/'alias';alias.symlink_to(foreign,target_is_directory=True)
        plain_file=self.f.repo/'not-directory';plain_file.write_bytes(b'preserve file')
        for parent in (alias/'nested',plain_file/'nested'):
            app=parent/'hex.app'
            with patch.object(fixtures.local_ci,'LocalCiRunner',return_value=self.runner), \
                    patch.object(delivery,'assert_inactive'),patch.object(restart,'NativeController') as controller:
                with self.assertRaises((delivery.DeliveryError,OSError)):
                    restart.inspect_recreated_target(self.f.repo,app,self.source,fixtures.local_ci,delivery,Mock())
            controller.assert_not_called();self.assertEqual(self.owner.read_bytes(),raw)
        self.assertFalse((foreign/'nested').exists());self.assertEqual(plain_file.read_bytes(),b'preserve file')


class PreflightTests(ReleaseFixture):
    def prepare_fixture(self):
        # This repository already exists under our exclusively owned temporary
        # directory. macOS may spell that directory through /var -> /private/var.
        # Derive the fixture app from its real root; production paths stay strict.
        self.f.repo = self.f.repo.resolve()
        tool=self.f.bin/'git'
        tool.write_text(tool.read_text().replace('deadbeef',self.source))
        self.app=self.f.repo/'artifacts/hex.app'
        self.controller=Mock();self.controller.snapshot.return_value=[]
        self.factory=Mock(return_value=self.controller)
        self.raw=Mock()
        self.stack=ExitStack();self.addCleanup(self.stack.close)
        self.stack.enter_context(patch.object(fixtures.local_ci,'LocalCiRunner',return_value=self.runner))
        self.stack.enter_context(patch.object(delivery,'assert_inactive'))
        self.stack.enter_context(patch.dict(os.environ,{},clear=True))

    def prepare(self):
        return restart.prepare(self.f.repo,self.app,self.source,fixtures.local_ci,delivery,self.raw,
                               controller_factory=self.factory)

    def test_real_owned_metadata_recovery_is_idempotent_preserves_owner_and_user_files(self):
        self.prepare_fixture()
        workspace=delivery.install_workspace(self.app);workspace.mkdir(parents=True)
        (workspace/'.DS_Store').write_bytes(b'Finder');user=self.app.parent/'song.hex';user.write_bytes(b'keep')
        raw=self.owner.read_bytes()
        self.assertEqual(self.prepare(),hashlib.sha256(raw).hexdigest())
        self.assertFalse(workspace.exists());self.assertEqual(user.read_bytes(),b'keep')
        self.assertEqual(self.owner.read_bytes(),raw);self.assertIsNone(self.runner.target_lock)
        self.assertEqual(self.prepare(),hashlib.sha256(raw).hexdigest())
        self.controller.request.assert_not_called()
        self.assertEqual(self.controller.close.call_count,2)
        reports=list((self.f.repo/'artifacts/launch').glob('restart-*/restart-preflight.json'))
        self.assertEqual(len(reports),2)
        self.assertTrue(all(json.loads(p.read_text())['status']=='ready_for_build' for p in reports))

    def test_supported_configured_override_uses_same_verified_preflight(self):
        self.prepare_fixture()
        config=self.f.repo/'scripts/hex-test-build.json'; config.parent.mkdir(parents=True,exist_ok=True)
        config.write_text('{"app_path":"artifacts/hex.app"}')
        self.app=self.f.repo/'configured/Hex.app'
        raw=self.owner.read_bytes()
        with patch.dict(os.environ,{'HEX_DEV_APP_PATH':str(self.app)}):
            self.assertEqual(self.prepare(),hashlib.sha256(raw).hexdigest())
        self.factory.assert_called_once()
        self.controller.request.assert_not_called()
        self.assertEqual(self.owner.read_bytes(),raw)
        self.assertIsNone(self.runner.target_lock)

    def test_configured_override_does_not_bypass_alias_or_foreign_owner_refusal(self):
        self.prepare_fixture();raw=self.owner.read_bytes()
        self.app.parent.mkdir(parents=True)
        alias=self.f.repo/'alias-parent';alias.symlink_to(self.app.parent,target_is_directory=True)
        original=self.app;self.app=alias/'hex.app'
        with patch.dict(os.environ,{'HEX_DEV_APP_PATH':str(self.app)}):
            with self.assertRaisesRegex(delivery.DeliveryError,'symlink'):self.prepare()
        self.app=original;self.record['workspace_owner']['workspace_root']='/foreign';self.save()
        foreign=self.owner.read_bytes()
        with patch.dict(os.environ,{'HEX_DEV_APP_PATH':str(self.app)}):
            with self.assertRaises(restart.RestartError):self.prepare()
        self.factory.assert_not_called()
        self.assertEqual(self.owner.read_bytes(),foreign)
        self.assertIsNone(self.runner.target_lock)
        self.owner.write_bytes(raw)

    def test_first_install_absent_bundle_does_not_claim_close_hex(self):
        self.prepare_fixture();self.owner.unlink()
        self.assertIsNone(self.prepare());self.controller.request.assert_not_called()

    def test_busy_activation_and_target_locks_refuse_before_process_control(self):
        self.prepare_fixture();self.app.parent.mkdir(parents=True)
        with delivery.activation_lock(self.app):
            with self.assertRaisesRegex(delivery.DeliveryError,'activation owns'):self.prepare()
        self.factory.assert_not_called()
        lockpath,_=fixtures.local_ci.target_state_paths(self.target,self.runner.target_state_dir)
        held=fixtures.local_ci.CargoTargetLock(lockpath,{'operation':'fixture-other-build'})
        held.acquire()
        try:
            with self.assertRaises(fixtures.local_ci.TargetCacheError):self.prepare()
        finally:held.release()
        self.factory.assert_not_called();self.assertEqual(self.read(),hashlib.sha256(self.owner.read_bytes()).hexdigest())

    def test_pending_admission_refuses_before_process_control(self):
        self.prepare_fixture()
        lockpath,_=fixtures.local_ci.target_state_paths(self.target,self.runner.target_state_dir)
        pending=fixtures.local_ci.target_admission_pending_path(lockpath)
        pending.write_bytes(b'incomplete admission')
        raw=self.owner.read_bytes()
        with self.assertRaisesRegex(fixtures.local_ci.TargetCacheError,'admission is incomplete'):
            self.prepare()
        self.factory.assert_not_called()
        self.assertEqual(self.owner.read_bytes(),raw)
        self.assertEqual(pending.read_bytes(),b'incomplete admission')
        self.assertIsNone(self.runner.target_lock)

    def test_foreign_owner_and_unknown_install_files_and_legacy_journal_preserved_before_quit(self):
        self.prepare_fixture();raw=self.owner.read_bytes()
        self.record['workspace_owner']['workspace_root']='/foreign';self.save()
        with self.assertRaises(restart.RestartError):self.prepare()
        self.owner.write_bytes(raw)
        workspace=delivery.install_workspace(self.app);workspace.mkdir(parents=True)
        unknown=workspace/'project.hex';unknown.write_bytes(b'untouched')
        with self.assertRaises(delivery.DeliveryError):self.prepare()
        self.assertEqual(unknown.read_bytes(),b'untouched');unknown.unlink();workspace.rmdir()
        journal=self.app.parent/'.hex-test-build-transaction.json';journal.write_bytes(b'old')
        with self.assertRaisesRegex(restart.RestartError,'legacy'):self.prepare()
        self.assertEqual(journal.read_bytes(),b'old');self.factory.assert_not_called()
        self.assertEqual(self.owner.read_bytes(),raw)

    def test_alias_application_path_still_refuses_without_process_or_state_mutation(self):
        self.prepare_fixture();self.app.parent.mkdir(parents=True)
        sentinel=self.app.parent/'song.hex';sentinel.write_bytes(b'preserve user file')
        alias=self.f.repo/'fixture-app-alias';alias.symlink_to(self.app.parent,target_is_directory=True)
        self.app=alias/'hex.app';raw=self.owner.read_bytes()
        with self.assertRaisesRegex(delivery.DeliveryError,'refusing symlink'):
            self.prepare()
        self.factory.assert_not_called()
        self.assertEqual(self.owner.read_bytes(),raw)
        self.assertEqual(sentinel.read_bytes(),b'preserve user file')
        self.assertTrue(alias.is_symlink())

    def test_interrupt_preserves_install_workspace_pending_owner_and_releases_locks(self):
        self.prepare_fixture();workspace=delivery.install_workspace(self.app);workspace.mkdir(parents=True)
        metadata=workspace/'.DS_Store';metadata.write_bytes(b'Finder');raw=self.owner.read_bytes()
        identity=restart.Identity(123,'start',str(self.app),str(self.app/'Contents/MacOS/hex'),'org.portalsurfer.hex.dev', 1, 2, 1, 3)
        self.controller.snapshot.return_value=[identity]
        with patch.object(restart,'stop_exact',side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):self.prepare()
        self.assertEqual(self.owner.read_bytes(),raw);self.assertEqual(metadata.read_bytes(),b'Finder')
        self.assertIsNone(self.runner.target_lock)
        self.controller.close.assert_called_once()
        report=next((self.f.repo/'artifacts/launch').glob('restart-*/restart-preflight.json'))
        self.assertEqual(json.loads(report.read_text())['status'],'interrupted')
        with delivery.activation_lock(self.app):pass



if __name__=='__main__':unittest.main()
