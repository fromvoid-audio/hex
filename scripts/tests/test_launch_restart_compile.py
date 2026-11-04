"""Compiled-control tests use only compiler/helper children created by this fixture."""
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import launch_restart_support
restart = launch_restart_support.restart


class CompiledHelperTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        (self.root / 'scripts').mkdir()
        self.source = self.root / 'scripts/launch-restart.swift'
        self.source.write_bytes((HERE.parent / 'launch-restart.swift').read_bytes())
        self.output = self.root / 'output'; self.output.mkdir(mode=0o700)
        self.app = self.root / 'hex.app'
        self.config = self.root / 'compiler-config.json'; self.config.write_text('{}')
        self.compiler_log = self.root / 'compiler-calls.jsonl'
        self.native_log = self.root / 'native-calls.jsonl'
        self.compiler = self.root / 'fixture-compiler'
        native = ('#!' + sys.executable + '\nimport json,sys\n'
                  + 'with open(' + repr(str(self.native_log)) + ',"a") as f: f.write(json.dumps(sys.argv)+"\\n")\n'
                  + 'import pathlib,time\nconfig=json.loads(pathlib.Path(' + repr(str(self.config)) + ').read_text())\n'
                  + 'time.sleep(config.get("native_sleep",0))\n'
                  + 'if config.get("native_fail"): sys.exit(9)\n'
                  + 'print("not json" if config.get("bad_json") else "[]" if sys.argv[1]=="snapshot" else "true")\n')
        self.compiler.write_text('#!' + sys.executable + '\n'
            'import json,os,pathlib,sys,time\n'
            'with open(' + repr(str(self.compiler_log)) + ',"a") as f: f.write(json.dumps(sys.argv)+"\\n")\n'
            'config=json.loads(pathlib.Path(' + repr(str(self.config)) + ').read_text())\n'
            'pathlib.Path(' + repr(str(self.root/'compiler-pid')) + ').write_text(str(os.getpid()))\n'
            'if config.get("spawn_child"):\n'
            ' import subprocess\n'
            ' child=subprocess.Popen([sys.executable,"-c","import time;time.sleep(60)"],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)\n'
            ' pathlib.Path(' + repr(str(self.root/'compiler-child-pid')) + ').write_text(str(child.pid))\n'
            'time.sleep(config.get("sleep",0))\n'
            'if config.get("fail"): print("fixture compiler refused",file=sys.stderr);sys.exit(7)\n'
            'if config.get("change_installed"):\n'
            ' app=pathlib.Path(' + repr(str(self.app)) + ');exe=app/"Contents/MacOS/hex"\n'
            ' kind=config["change_installed"]\n'
            ' if kind=="bundle":\n'
            '  old=app.with_name("old-bundle");app.rename(old);exe.parent.mkdir(parents=True);os.link(old/"Contents/MacOS/hex",exe)\n'
            ' elif kind=="inode":\n'
            '  old=exe.with_name("old-executable");exe.rename(old);exe.write_bytes(old.read_bytes())\n'
            ' else: exe.write_bytes(b"different installed bytes")\n'
            'out=pathlib.Path(sys.argv[sys.argv.index("-o")+1])\n'
            'if config.get("missing"): sys.exit(0)\n'
            'if config.get("symlink"): out.symlink_to(config["symlink"]);sys.exit(0)\n'
            'if config.get("hardlink"): os.link(config["hardlink"],out);sys.exit(0)\n'
            'if config.get("directory"): out.mkdir();sys.exit(0)\n'
            'if config.get("change_original"): pathlib.Path(' + repr(str(self.source)) + ').write_text("changed source")\n'
            'if config.get("change_copy"): pathlib.Path(sys.argv[sys.argv.index("-o")-1]).write_text("changed copy")\n'
            'out.write_text(' + repr(native) + ')\n'
            'out.chmod(0o700)\n')
        self.compiler.chmod(0o700)
        self.stack = ExitStack(); self.addCleanup(self.stack.close)
        self.stack.enter_context(patch.object(restart, 'SWIFTC', str(self.compiler), create=True))
        self.controller = restart.NativeController(self.root, self.app, self.output, Mock())
        self.addCleanup(lambda: getattr(self.controller, 'close', lambda: None)())

    def calls(self, path):
        return [json.loads(x) for x in path.read_text().splitlines()] if path.exists() else []

    def test_native_calls_compile_once_instead_of_starting_interpreter_per_call(self):
        # The parent path executes /usr/bin/swift for BOTH requests, so this
        # fixture makes its real protocol failure observable without running Swift.
        with patch.object(restart.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '[]', '')) as interpreted:
            self.assertEqual(self.controller.snapshot(), [])
            self.assertEqual(self.controller.snapshot(), [])
        self.assertEqual(interpreted.call_count, 0, 'native calls still include per-call Swift compilation/startup')
        self.assertEqual(len(self.calls(self.compiler_log)), 1)
        native = self.calls(self.native_log)
        self.assertEqual(len(native), 2)
        self.assertEqual(native[0][0], native[1][0])
        self.assertNotEqual(native[0][0], '/usr/bin/swift')
        self.assertEqual([x[1] for x in native], ['snapshot', 'snapshot'])

    def test_hardlinked_compiler_reuses_one_binary_for_snapshot_quit_force(self):
        for index in range(77):
            os.link(self.compiler, self.root / ('compiler-link-' + str(index)))
        self.assertEqual(self.compiler.stat().st_nlink, 78)
        self.test_snapshot_quit_force_all_use_same_binary_and_repeat_installed_seal_after_compile()
        report = json.loads((self.output / 'native-helper.json').read_text())
        self.assertEqual(report['compiler'], str(self.compiler))
        self.assertEqual(report['compiler_identity'][-1], hashlib.sha256(self.compiler.read_bytes()).hexdigest())
        self.assertEqual(report['compile_timeout_seconds'], 120)
        self.assertEqual(report['operation_timeout_seconds'], 15)
        binary = Path(report['binary'])
        self.controller.close()
        self.assertFalse(binary.exists())
        self.assertTrue(self.compiler.exists())
        self.assertEqual(self.compiler.stat().st_nlink, 78)

    def test_compiler_identity_accepts_single_and_multiple_links_only_for_compiler(self):
        for count in (1, 78):
            with self.subTest(links=count):
                if count == 78:
                    for index in range(77):
                        os.link(self.compiler, self.root / ('identity-link-' + str(index)))
                identity = self.controller._compiler_identity()
                self.assertEqual(identity[-1], hashlib.sha256(self.compiler.read_bytes()).hexdigest())
                if count > 1:
                    with self.assertRaisesRegex(restart.RestartError, 'ambiguous'):
                        self.controller._file_identity(self.compiler, executable=True, owned=False)
        self.assertEqual(self.calls(self.compiler_log), [])
        self.assertEqual(self.calls(self.native_log), [])

    def test_zero_link_file_is_rejected_for_both_compiler_and_private_identity(self):
        fields = list(self.compiler.stat())
        fields[3] = 0  # st_nlink: an unlinked descriptor is never an accepted tool.
        detached = os.stat_result(fields)
        with patch.object(restart.os, 'fstat', return_value=detached):
            with self.assertRaisesRegex(restart.RestartError, 'ambiguous'):
                self.controller._compiler_identity()
            with self.assertRaisesRegex(restart.RestartError, 'ambiguous'):
                self.controller._file_identity(self.compiler, executable=True)
        self.assertIsNone(self.controller._helper_dir)
        self.assertEqual(self.calls(self.compiler_log), [])
        self.assertEqual(self.calls(self.native_log), [])

    def test_compiler_unsafe_paths_refuse_before_compile_and_latch(self):
        candidates = []
        missing = self.root / 'missing-tool'
        candidates.append(missing)
        directory = self.root / 'directory-tool'; directory.mkdir()
        candidates.append(directory)
        fifo = self.root / 'fifo-tool'; os.mkfifo(fifo)
        candidates.append(fifo)
        link = self.root / 'symlink-tool'; link.symlink_to(self.compiler)
        candidates.append(link)
        parent_link = self.root / 'parent-link'; parent_link.symlink_to(self.root, target_is_directory=True)
        candidates.append(parent_link / self.compiler.name)
        for mode in (0o720, 0o702, 0o600):
            tool = self.root / ('mode-tool-' + oct(mode))
            tool.write_bytes(self.compiler.read_bytes()); tool.chmod(mode)
            candidates.append(tool)
        for index, tool in enumerate(candidates):
            with self.subTest(tool=tool.name, parent=tool.parent.name):
                output = self.root / ('unsafe-output-' + str(index)); output.mkdir(mode=0o700)
                controller = restart.NativeController(self.root, self.app, output, Mock())
                with patch.object(restart, 'SWIFTC', str(tool)), patch.object(controller, '_run_owned') as run:
                    with self.assertRaises(restart.RestartError): controller.snapshot()
                    with self.assertRaisesRegex(restart.RestartError, 'closed'): controller.snapshot()
                    run.assert_not_called()
                self.assertIsNone(controller._helper_dir)
                self.assertEqual(list(output.iterdir()), [])

    def test_private_original_source_hardlink_refused_before_compile(self):
        os.link(self.source, self.root / 'source-alias')
        with patch.object(self.controller, '_run_owned') as run:
            with self.assertRaisesRegex(restart.RestartError, 'ambiguous'): self.controller.snapshot()
            run.assert_not_called()
        self.assertIsNone(self.controller._helper_dir)

    def test_private_source_snapshot_hardlink_refused_before_operation(self):
        run = self.controller._run_owned
        def link_snapshot(argv, timeout, phase):
            result = run(argv, timeout, phase)
            if phase == 'compile':
                os.link(self.controller._helper_dir / 'launch-restart.swift', self.root / 'snapshot-alias')
            return result
        with patch.object(self.controller, '_run_owned', side_effect=link_snapshot):
            with self.assertRaisesRegex(restart.RestartError, 'ambiguous'): self.controller.snapshot()
        self.assertEqual(len(self.calls(self.compiler_log)), 1)
        self.assertEqual(self.calls(self.native_log), [])
        self.assertFalse(list(self.output.glob('native-helper-*')))

    def test_compiler_mutation_during_compile_refuses_before_operation(self):
        original = self.compiler.read_bytes()
        for index, change in enumerate(('inode', 'content', 'mode', 'links')):
            with self.subTest(change=change):
                tool = self.root / ('changing-tool-' + str(index))
                tool.write_bytes(original); tool.chmod(0o700)
                output = self.root / ('changing-output-' + str(index)); output.mkdir(mode=0o700)
                controller = restart.NativeController(self.root, self.app, output, Mock())
                run = controller._run_owned
                def mutate(argv, timeout, phase):
                    result = run(argv, timeout, phase)
                    if phase == 'compile':
                        if change == 'inode':
                            replacement = self.root / 'replacement-tool'
                            replacement.write_bytes(original); replacement.chmod(0o700)
                            replacement.replace(tool)
                        elif change == 'content': tool.write_bytes(original + b'\n# changed\n')
                        elif change == 'mode': tool.chmod(0o500)
                        else: os.link(tool, self.root / 'new-compiler-alias')
                    return result
                with patch.object(restart, 'SWIFTC', str(tool)), patch.object(controller, '_run_owned', side_effect=mutate):
                    with self.assertRaisesRegex(restart.RestartError, 'compiler changed'): controller.snapshot()
                    with self.assertRaisesRegex(restart.RestartError, 'closed'): controller.snapshot()
                self.assertEqual(self.calls(self.native_log), [])
                self.assertEqual(len(self.calls(self.compiler_log)), index + 1)
                self.assertFalse(list(output.glob('native-helper-*')))

    def test_compiler_changes_during_identity_hash_are_refused(self):
        original_sha256 = restart.hashlib.sha256
        for index, change in enumerate(('path', 'links', 'content')):
            with self.subTest(change=change):
                tool = self.root / ('hashing-tool-' + str(index))
                tool.write_bytes(self.compiler.read_bytes()); tool.chmod(0o700)
                class ChangingDigest:
                    def __init__(inner):
                        inner.digest = original_sha256()
                        inner.changed = False
                    def update(inner, block):
                        inner.digest.update(block)
                        if not inner.changed:
                            inner.changed = True
                            if change == 'path':
                                replacement = self.root / 'hash-replacement'
                                replacement.write_bytes(tool.read_bytes()); replacement.chmod(0o700)
                                replacement.replace(tool)
                            elif change == 'links': os.link(tool, self.root / 'hash-link')
                            else: tool.write_bytes(b'changed')
                    def hexdigest(inner): return inner.digest.hexdigest()
                with patch.object(restart, 'SWIFTC', str(tool)), patch.object(restart.hashlib, 'sha256', ChangingDigest):
                    with self.assertRaisesRegex(restart.RestartError, 'changed while inspected'):
                        self.controller._compiler_identity()
        self.assertIsNone(self.controller._helper_dir)
        self.assertEqual(self.calls(self.compiler_log), [])
        self.assertEqual(self.calls(self.native_log), [])

    def configure(self, **values):
        self.config.write_text(json.dumps(values))

    def test_lazy_compile_default_budgets_provenance_and_only_own_cleanup(self):
        self.assertEqual(self.calls(self.compiler_log), [])
        self.assertIsNone(self.controller._helper_dir)
        self.controller.snapshot()
        report=json.loads((self.output/'native-helper.json').read_text())
        directory=self.controller._helper_dir
        self.assertEqual(directory.stat().st_mode & 0o777,0o700)
        self.assertEqual(report['source_sha256'],hashlib.sha256(self.source.read_bytes()).hexdigest())
        self.assertEqual(report['binary_sha256'],hashlib.sha256(Path(report['binary']).read_bytes()).hexdigest())
        self.assertEqual(report['compile_timeout_seconds'],120)
        self.assertEqual(report['operation_timeout_seconds'],15)
        sentinel=self.output/'keep-evidence';sentinel.write_bytes(b'preserve')
        self.controller.close();self.controller.close()
        self.assertFalse(directory.exists())
        self.assertEqual(sentinel.read_bytes(),b'preserve')
        self.assertTrue((self.output/'native-helper.json').exists())
        with self.assertRaisesRegex(restart.RestartError,'closed'):self.controller.snapshot()
        self.assertEqual(len(self.calls(self.compiler_log)),1)

    def test_cold_compile_has_separate_budget_then_fast_operations_keep_runtime_budget(self):
        import time
        self.configure(sleep=.35)
        real_popen=subprocess.Popen;wait_budgets=[]
        def spawn(*args,**kwargs):
            child=real_popen(*args,**kwargs);wait=child.wait
            def record(timeout=None):
                wait_budgets.append(timeout)
                return wait(timeout=timeout)
            child.wait=record
            return child
        # This is a budget-routing oracle, not an interpreter startup benchmark.
        # Generous fixture budgets keep cold Python startup out of the assertion;
        # recording the actual waits detects merged/per-call compile budgets.
        with patch.object(restart,'COMPILE_TIMEOUT',30),patch.object(restart,'OPERATION_TIMEOUT',5),patch.object(restart.subprocess,'Popen',side_effect=spawn):
            start=time.monotonic();self.assertEqual(self.controller.snapshot(),[])
            self.assertGreater(time.monotonic()-start,.35)
            self.assertEqual(self.controller.snapshot(),[])
        self.assertEqual(wait_budgets,[30,5,5])
        self.assertEqual(len(self.calls(self.compiler_log)),1)
        self.assertEqual(len(self.calls(self.native_log)),2)
        self.assertIn('budget 30s',(self.output/'native-compile.log').read_text())
        self.assertIn('budget 5s',(self.output/'native-control.log').read_text())

    def test_snapshot_quit_force_all_use_same_binary_and_repeat_installed_seal_after_compile(self):
        self.controller.snapshot()
        self.controller.seal=(1,2,'sealed')
        executable=self.app/'Contents/MacOS/hex'; executable.parent.mkdir(parents=True)
        executable.write_bytes(b'fixture executable')
        identity=restart.Identity(123,'fixture',str(self.app),str(executable),'org.portalsurfer.hex.dev',
                                  *restart._filesystem_identity(self.app, executable))
        with patch.object(self.controller,'disk_identity',return_value=self.controller.seal) as seal:
            self.assertTrue(self.controller.request('quit',identity))
            self.assertTrue(self.controller.request('force',identity))
        self.assertEqual(seal.call_count,4)
        self.assertEqual(len(self.calls(self.compiler_log)),1)
        native=self.calls(self.native_log)
        self.assertEqual([x[1] for x in native],['snapshot','quit','force'])
        self.assertEqual(len({x[0] for x in native}),1)
        for x in native[1:]:self.assertEqual(json.loads(x[3])['started'],'fixture')

    def test_installed_bundle_change_during_cold_compile_refuses_before_operation(self):
        self.controller.seal=(1,2,'old')
        with patch.object(self.controller,'disk_identity',side_effect=[self.controller.seal,(1,3,'new')]):
            with self.assertRaisesRegex(restart.RestartError,'installed application changed'):
                self.controller.call('force')
        self.assertEqual(len(self.calls(self.compiler_log)),1)
        self.assertEqual(self.calls(self.native_log),[])
        self.assertFalse(list(self.output.glob('native-helper-*')))

    def test_actual_installed_replacements_during_compile_never_start_native_operation(self):
        for kind in ('bundle','inode','content'):
            with self.subTest(kind=kind):
                app=self.app;exe=app/'Contents/MacOS/hex'
                exe.parent.mkdir(parents=True,exist_ok=True);exe.write_bytes(b'original fixture bytes')
                output=self.root/('replacement-'+kind);output.mkdir(mode=0o700)
                delivery=Mock();delivery.sha.side_effect=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
                controller=restart.NativeController(self.root,app,output,delivery)
                pins=(app.stat().st_dev,app.stat().st_ino,exe.stat().st_dev,exe.stat().st_ino)
                expected=restart.Identity(123,'fixture',str(app),str(exe),'org.portalsurfer.hex.dev',*pins)
                self.configure(change_installed=kind)
                with self.assertRaisesRegex(restart.RestartError,'installed application changed'):
                    controller.request('quit',expected)
                self.assertEqual(self.calls(self.native_log),[])
                self.assertFalse(list(output.glob('native-helper-*')))
                with self.assertRaisesRegex(restart.RestartError,'closed'):controller.snapshot()
        self.assertEqual(len(self.calls(self.compiler_log)),3)

    def test_missing_compiler_refuses_without_fallback_or_native_operation(self):
        with patch.object(restart,'SWIFTC',str(self.root/'missing-compiler')):
            with self.assertRaisesRegex(restart.RestartError,'toolchain unavailable'):self.controller.snapshot()
        with self.assertRaisesRegex(restart.RestartError,'closed'):self.controller.snapshot()
        self.assertEqual(self.calls(self.native_log),[])
        self.assertEqual(self.calls(self.compiler_log),[])

    def test_compiler_failure_latches_no_retry_cleans_workspace_and_keeps_diagnostic(self):
        self.configure(fail=True)
        with self.assertRaisesRegex(restart.RestartError,'compile refused'):self.controller.snapshot()
        self.assertFalse(list(self.output.glob('native-helper-*')))
        self.assertIn('fixture compiler refused',(self.output/'native-compile.log').read_text())
        attempt=json.loads((self.output/'native-compile.json').read_text())
        self.assertEqual(attempt['source_sha256'],hashlib.sha256(self.source.read_bytes()).hexdigest())
        self.assertFalse((self.output/'native-helper.json').exists())
        self.assertEqual(self.calls(self.native_log),[])
        self.configure()
        with self.assertRaisesRegex(restart.RestartError,'closed'):self.controller.snapshot()
        self.assertEqual(len(self.calls(self.compiler_log)),1)

    def test_missing_output_is_not_adopted_after_successful_compiler_exit(self):
        self.configure(missing=True)
        with self.assertRaises(restart.RestartError):self.controller.snapshot()
        self.assertEqual(self.calls(self.native_log),[])
        self.assertFalse(list(self.output.glob('native-helper-*')))

    def test_symlink_output_is_refused_without_changing_its_foreign_target(self):
        foreign=self.root/'unrelated-executable';foreign.write_bytes(b'untouched');foreign.chmod(0o700)
        self.configure(symlink=str(foreign))
        with self.assertRaisesRegex(restart.RestartError,'symlink'):self.controller.snapshot()
        self.assertEqual(foreign.read_bytes(),b'untouched');self.assertEqual(self.calls(self.native_log),[])
        self.assertFalse(list(self.output.glob('native-helper-*')))

    def test_hardlinked_output_and_directory_output_are_not_valid_binaries(self):
        # Each invocation has its own output/evidence directory; no failed retry.
        foreign=self.root/'unrelated';foreign.write_bytes(b'keep');foreign.chmod(0o700)
        self.configure(hardlink=str(foreign))
        with self.assertRaisesRegex(restart.RestartError,'ambiguous'):self.controller.snapshot()
        self.assertEqual(foreign.read_bytes(),b'keep')
        output=self.root/'second-output';output.mkdir(mode=0o700)
        self.controller=restart.NativeController(self.root,self.app,output,Mock())
        self.configure(directory=True)
        with self.assertRaisesRegex(restart.RestartError,'ambiguous'):self.controller.snapshot()
        self.assertEqual(self.calls(self.native_log),[])

    def test_source_changes_during_compile_are_refused_before_native_operation(self):
        self.configure(change_original=True)
        with self.assertRaisesRegex(restart.RestartError,'source or executable changed'):self.controller.snapshot()
        self.assertEqual(self.calls(self.native_log),[])
        self.assertFalse(list(self.output.glob('native-helper-*')))

    def test_snapshot_source_change_during_compile_is_refused(self):
        self.configure(change_copy=True)
        with self.assertRaisesRegex(restart.RestartError,'source or executable changed'):self.controller.snapshot()
        self.assertEqual(self.calls(self.native_log),[])

    def test_binary_byte_change_between_calls_refuses_without_recompile(self):
        self.controller.snapshot();binary=self.controller._binary
        binary.write_bytes(binary.read_bytes()+b'\n# mutation\n')
        with self.assertRaisesRegex(restart.RestartError,'source or executable changed'):self.controller.snapshot()
        self.assertEqual(len(self.calls(self.native_log)),1)
        self.assertEqual(len(self.calls(self.compiler_log)),1)

    def test_same_bytes_different_binary_inode_is_not_adopted(self):
        self.controller.snapshot();binary=self.controller._binary
        replacement=binary.with_name('replacement');replacement.write_bytes(binary.read_bytes());replacement.chmod(0o700)
        replacement.replace(binary)
        with self.assertRaisesRegex(restart.RestartError,'source or executable changed'):self.controller.snapshot()
        self.assertEqual(len(self.calls(self.native_log)),1)

    def test_binary_symlink_between_calls_preserves_unrelated_file(self):
        self.controller.snapshot();binary=self.controller._binary
        foreign=self.root/'keep';foreign.write_bytes(b'untouched');binary.unlink();binary.symlink_to(foreign)
        with self.assertRaisesRegex(restart.RestartError,'symlink'):self.controller.snapshot()
        self.assertEqual(foreign.read_bytes(),b'untouched')
        self.assertEqual(len(self.calls(self.native_log)),1)

    def test_original_source_replacement_between_calls_refuses_even_same_bytes(self):
        self.controller.snapshot()
        replacement=self.source.with_suffix('.replacement');replacement.write_bytes(self.source.read_bytes());replacement.replace(self.source)
        with self.assertRaisesRegex(restart.RestartError,'source or executable changed'):self.controller.snapshot()
        self.assertEqual(len(self.calls(self.native_log)),1)

    def test_replaced_workspace_never_deletes_foreign_directory(self):
        self.controller.snapshot();directory=self.controller._helper_dir
        moved=directory.with_name(directory.name+'-moved');directory.rename(moved)
        foreign=self.root/'foreign';foreign.mkdir();keep=foreign/'user.hex';keep.write_bytes(b'preserve')
        directory.symlink_to(foreign,target_is_directory=True)
        try:
            with self.assertRaisesRegex(restart.RestartError,'workspace identity changed'):self.controller.snapshot()
            self.assertEqual(keep.read_bytes(),b'preserve')
            self.assertTrue(moved.exists())
            self.assertEqual(len(self.calls(self.native_log)),1)
        finally:
            directory.unlink();moved.rename(directory)

    def test_compile_timeout_stops_owned_compiler_and_descendant_not_unrelated_child(self):
        import time
        unrelated=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'])
        self.addCleanup(lambda: (unrelated.kill(),unrelated.wait()) if unrelated.poll() is None else None)
        self.configure(sleep=60,spawn_child=True)
        real_popen=subprocess.Popen;compiler_children=[];compile_waits=[]
        def spawn(*args,**kwargs):
            child=real_popen(*args,**kwargs)
            compiler_children.append(child);wait=child.wait;first_wait=True
            def after_descendant_ready(timeout=None):
                nonlocal first_wait
                if first_wait:
                    first_wait=False
                    # Fixture startup is separate from the short timeout oracle.
                    # Production already owns this process before readiness can
                    # fail, so its exception path still kills/reaps the group.
                    deadline=time.monotonic()+15
                    markers=[self.root/name for name in ('compiler-pid','compiler-child-pid')]
                    while not all(path.exists() and path.read_text().isdigit() for path in markers):
                        self.assertIsNone(child.poll(),'fixture compiler exited before descendant readiness')
                        self.assertLess(time.monotonic(),deadline,'fixture compiler never recorded both process IDs')
                        time.sleep(.01)
                    compile_waits.append(timeout)
                return wait(timeout=timeout)
            child.wait=after_descendant_ready
            return child
        with patch.object(restart,'COMPILE_TIMEOUT',.3),patch.object(restart.subprocess,'Popen',side_effect=spawn):
            with self.assertRaisesRegex(restart.RestartError,'compile timed out'):self.controller.snapshot()
        self.assertEqual(compile_waits,[.3])
        self.assertEqual(len(compiler_children),1)
        self.assertIsNotNone(compiler_children[0].poll(),'timed-out owned compiler was not reaped')
        self.assertIsNone(unrelated.poll())
        self.assertEqual(self.calls(self.native_log),[])
        self.assertFalse(list(self.output.glob('native-helper-*')))
        for name in ['compiler-pid','compiler-child-pid']:
            pid=int((self.root/name).read_text())
            # A dead reparented child can briefly remain a zombie on Linux.
            result=subprocess.run(['ps','-p',str(pid),'-o','stat='],capture_output=True,text=True,check=False)
            self.assertTrue(result.returncode!=0 or result.stdout.strip().startswith('Z'),(pid,result.stdout))

    def test_interrupt_stops_owned_compiler_and_prevents_native_operation(self):
        import time
        self.configure(sleep=60)
        real_popen=subprocess.Popen;children=[]
        def spawn(*args,**kwargs):
            child=real_popen(*args,**kwargs);children.append(child);wait=child.wait
            def interrupt(timeout=None):
                if timeout==120:
                    time.sleep(.1)
                    raise KeyboardInterrupt
                return wait(timeout=timeout)
            child.wait=interrupt
            return child
        with patch.object(restart.subprocess,'Popen',side_effect=spawn):
            with self.assertRaises(KeyboardInterrupt):self.controller.snapshot()
        self.assertEqual(len(children),1);self.assertIsNotNone(children[0].poll())
        self.assertEqual(self.calls(self.native_log),[])
        self.assertFalse(list(self.output.glob('native-helper-*')))
        self.assertIn('interrupted',(self.output/'native-compile.log').read_text())

    def test_native_operation_retains_its_own_timeout_and_never_recompiles(self):
        import time
        self.configure(native_sleep=60)
        real_popen=subprocess.Popen;native_children=[];runtime_waits=[]
        def spawn(*args,**kwargs):
            child=real_popen(*args,**kwargs)
            if Path(args[0][0]).name=='launch-restart':
                native_children.append(child);wait=child.wait;first_wait=True
                def after_invocation(timeout=None):
                    nonlocal first_wait
                    if first_wait:
                        first_wait=False
                        # Fixture-only readiness: the owned Python helper must
                        # record its invocation before the short runtime oracle.
                        # Production's 15s deadline still includes all startup.
                        deadline=time.monotonic()+15
                        while not self.native_log.exists() or not self.native_log.read_text().endswith('\n'):
                            self.assertIsNone(child.poll(),'fixture helper exited before invocation')
                            self.assertLess(time.monotonic(),deadline,'fixture helper never recorded invocation')
                            time.sleep(.01)
                        runtime_waits.append(timeout)
                    return wait(timeout=timeout)
                child.wait=after_invocation
            return child
        with patch.object(restart,'OPERATION_TIMEOUT',.2),patch.object(restart.subprocess,'Popen',side_effect=spawn):
            with self.assertRaisesRegex(restart.RestartError,'snapshot timed out'):self.controller.snapshot()
        self.assertEqual(runtime_waits,[.2])
        self.assertEqual(len(native_children),1)
        self.assertIsNotNone(native_children[0].poll(),'timed-out owned helper was not reaped')
        self.assertEqual(len(self.calls(self.compiler_log)),1)
        self.assertEqual(len(self.calls(self.native_log)),1)
        self.assertIn('timed out',(self.output/'native-control.log').read_text())
        self.assertFalse(list(self.output.glob('native-helper-*')))
        with self.assertRaisesRegex(restart.RestartError,'closed'):self.controller.snapshot()
        self.assertEqual(len(self.calls(self.compiler_log)),1)
        self.assertEqual(len(self.calls(self.native_log)),1)

    def test_native_refusal_and_invalid_json_do_not_escape_as_success(self):
        self.configure(native_fail=True)
        with self.assertRaisesRegex(restart.RestartError,'app control refused'):self.controller.snapshot()
        output=self.root/'second-output';output.mkdir(mode=0o700)
        self.controller=restart.NativeController(self.root,self.app,output,Mock())
        self.configure(bad_json=True)
        with self.assertRaisesRegex(restart.RestartError,'invalid native'):self.controller.snapshot()
        self.assertEqual(len(self.calls(self.compiler_log)),2)


    def test_unconfirmed_child_cleanup_retains_workspace_and_forbids_retry(self):
        self.configure(sleep=60)
        real_popen=subprocess.Popen;children=[];waits=[]
        def spawn(*args,**kwargs):
            child=real_popen(*args,**kwargs);children.append(child);waits.append(child.wait)
            child.wait=lambda timeout=None: (_ for _ in ()).throw(subprocess.TimeoutExpired(args[0],timeout))
            return child
        try:
            with patch.object(restart.subprocess,'Popen',side_effect=spawn):
                with self.assertRaisesRegex(restart.RestartError,'exit is unconfirmed; workspace retained'):
                    self.controller.snapshot()
            self.assertTrue(self.controller._helper_dir.is_dir())
            self.assertEqual(self.calls(self.native_log),[])
            with self.assertRaisesRegex(restart.RestartError,'workspace retained'):self.controller.snapshot()
            self.assertEqual(len(children),1)
        finally:
            # Fixture-only recovery AFTER its actual owned child is confirmed
            # dead; production has no retry/adoption/automatic-clear operation.
            for child,wait in zip(children,waits):
                child.wait=wait;wait(timeout=5)
            self.controller._cleanup_blocked=False
            self.controller.close()


if __name__ == '__main__':
    unittest.main()
