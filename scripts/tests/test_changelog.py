"""Actual Git-range/adapter fixture checks; injected model responses are not live AI evidence."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('changelog', Path(__file__).resolve().parent.parent / 'changelog.py')
notes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notes)


class ChangelogTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='hex-changelog-test-')
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.git('init', '-q')
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion="0.1.0"\n')
        (self.root / 'feature.txt').write_text('first\n')
        self.base = self.commit('initial fixture')
        (self.root / 'feature.txt').write_text('first\nsecond\n')
        self.source = self.commit('fix clip selection fixture')
        self.calls = 0

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args], stderr=subprocess.DEVNULL).decode().strip()

    def commit(self, message):
        self.git('add', '.')
        self.git('-c', 'user.name=fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', message)
        return self.git('rev-parse', 'HEAD')

    def model(self, directory, timeout):
        self.calls += 1
        data = json.loads((directory / 'input.json').read_bytes())
        self.assertIn('+second', data['diff'])
        self.assertEqual(data['base'], self.base)
        return {'entries': [{'category': 'fixes', 'text': 'clip selection stays consistent.', 'commits': [self.source]}], 'reason': 'fixture response, not live inference'}

    def evidence(self, directory, result, execution, overrides=None, corrupt_manifest=False, link=False, duplicate=False, unmanifested=False):
        import io
        import tarfile
        payloads = {'result.json': result.read_bytes(), 'execution.json': execution.read_bytes()}
        payloads.update({'original-input/' + name: (directory / name).read_bytes()
                         for name in ('input.json', 'schema.json', 'prompt.txt', 'INPUTS.json')})
        if overrides:
            payloads.update(overrides)
        manifest = {name: {'bytes': len(raw), 'sha256': notes.digest(raw)} for name, raw in payloads.items()}
        if corrupt_manifest:
            manifest['result.json']['sha256'] = '0' * 64
        payloads['PAYLOADS.json'] = notes.encode(manifest)
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode='w:gz') as archive:
            for name, raw in payloads.items():
                member = tarfile.TarInfo(name)
                member.size = len(raw)
                archive.addfile(member, io.BytesIO(raw))
            if link:
                member = tarfile.TarInfo('foreign-link')
                member.type = tarfile.SYMTYPE
                member.linkname = '/foreign/user/file'
                archive.addfile(member)
            for name, raw in ([('result.json', result.read_bytes())] if duplicate else []) + ([('unmanifested-proof.json', b'{}')] if unmanifested else []):
                member = tarfile.TarInfo(name)
                member.size = len(raw)
                archive.addfile(member, io.BytesIO(raw))
        def git_input(args, raw):
            return subprocess.check_output(['git', '-C', str(self.root), *args], input=raw).decode().strip()
        blob = git_input(['hash-object', '-w', '--stdin'], buffer.getvalue())
        tree = git_input(['mktree'], f'100644 blob {blob}\tevidence.tar.gz\n'.encode())
        commit = git_input(['-c', 'user.name=fixture', '-c', 'user.email=fixture@example.invalid',
                            'commit-tree', tree, '-p', 'HEAD'], b'injected local evidence fixture, not live inference\n')
        return commit, 'evidence.tar.gz'

    def test_real_range_schema_export_and_source_identity(self):
        packet = notes.input_packet(self.root, self.base, self.source)
        out = self.root / 'packet'
        notes.export_packet(out, packet)
        manifest = json.loads((out / 'INPUTS.json').read_bytes())
        self.assertEqual(manifest['input_sha256'], notes.digest((out / 'input.json').read_bytes()))
        self.assertEqual(packet['commits'], [{'commit': self.source, 'subject': 'fix clip selection fixture'}])
        self.assertIn('UNTRUSTED DATA', (out / 'prompt.txt').read_text())

    def test_prepare_retains_exact_bytes_when_rebuilt_and_after_dataset_commit(self):
        self.assertTrue(notes.prepare(self.root, self.base, model=self.model))
        path = self.root / notes.DATASET
        before = path.read_bytes()
        self.commit('retain generated dataset')
        self.assertFalse(notes.prepare(self.root, model=self.model))
        self.assertFalse(notes.prepare(self.root, self.base, model=self.model))
        self.assertEqual(path.read_bytes(), before)
        self.assertEqual(self.calls, 1)
        notes.validate_current(self.root, 'HEAD')
        self.assertNotIn('diff', json.loads(before)['batches'][0]['input'], 'product data must not embed source patches')

    def test_offline_declared_range_is_valid_but_requested_new_end_is_stale(self):
        notes.prepare(self.root, self.base, model=self.model)
        self.commit('notes')
        (self.root / 'unrelated.txt').write_text('internal fixture\n')
        self.commit('unrelated later change')
        notes.validate_current(self.root)
        with self.assertRaisesRegex(notes.Refusal, 'requested end'):
            notes.validate_current(self.root, 'HEAD')

    def test_failure_invalid_json_unknown_reference_and_duplicate_preserve_valid_notes(self):
        notes.prepare(self.root, self.base, model=self.model)
        self.commit('notes')
        path = self.root / notes.DATASET
        previous = path.read_bytes()
        (self.root / 'feature.txt').write_text('third\n')
        current = self.commit('next fixture')
        def failure(*_):
            raise notes.Refusal('transport403 fixture')
        for model in [failure, lambda *_: {}, lambda *_: {'entries': [{'category': 'fixes', 'text': 'new note', 'commits': ['f' * 40]}], 'reason': ''},
                      lambda *_: {'entries': [{'category': 'fixes', 'text': 'clip selection stays consistent.', 'commits': [current]}], 'reason': ''}]:
            with self.assertRaises(notes.Refusal):
                notes.prepare(self.root, model=model)
            self.assertEqual(path.read_bytes(), previous)

    def test_actual_empty_classification_retains_prior_user_notes(self):
        notes.prepare(self.root, self.base, model=self.model)
        self.commit('notes')
        (self.root / 'internal.txt').write_text('internal fixture\n')
        self.commit('internal fixture')
        notes.prepare(self.root, model=lambda *_: {'entries': [], 'reason': 'injected internal-only classification'})
        value = notes.validate_current(self.root)
        self.assertEqual(len(value['batches']), 2)
        self.assertEqual(value['batches'][0]['result']['entries'][0]['text'], 'clip selection stays consistent.')
        self.assertEqual(value['batches'][1]['result']['entries'], [])

    def test_tampered_declared_identity_and_malformed_existing_file_are_refused(self):
        notes.prepare(self.root, self.base, model=self.model)
        self.commit('notes')
        path = self.root / notes.DATASET
        value = json.loads(path.read_bytes())
        value['batches'][0]['input']['prompt_sha256'] = '0' * 64
        path.write_bytes(notes.encode(value))
        with self.assertRaisesRegex(notes.Refusal, 'original named Git input'):
            notes.validate_current(self.root)
        path.write_bytes(b'{broken')
        with self.assertRaisesRegex(notes.Refusal, 'invalid changelog JSON'):
            notes.prepare(self.root, self.base, model=self.model)
        self.assertEqual(path.read_bytes(), b'{broken')

    def test_missing_baseline_dirty_source_and_oversize_input_do_not_infer(self):
        with self.assertRaisesRegex(notes.Refusal, 'explicit --base'):
            notes.prepare(self.root, model=self.model)
        (self.root / 'feature.txt').write_text('uncommitted\n')
        with self.assertRaisesRegex(notes.Refusal, 'commit candidate'):
            notes.prepare(self.root, self.base, model=self.model)
        (self.root / 'feature.txt').write_text('x' * notes.MAX_INPUT)
        self.commit('oversized fixture')
        with self.assertRaisesRegex(notes.Refusal, 'no input was truncated'):
            notes.prepare(self.root, self.base, model=self.model)
        self.assertEqual(self.calls, 0)

    def test_missing_notes_are_explicit_not_fresh_success(self):
        self.assertEqual(notes.validate_current(self.root, if_present=True)['batches'], [])
        with self.assertRaisesRegex(notes.Refusal, 'no generated changelog'):
            notes.validate_current(self.root, self.source, if_present=True)

    def test_real_subprocess_adapter_uses_existing_route_and_retains_failure(self):
        import sys
        packet = notes.input_packet(self.root, self.base, self.source)
        output = self.root / 'packet'
        notes.export_packet(output, packet)
        executable = self.root / 'configured-cli-fixture'
        executable.write_text('#!' + sys.executable + '\n' +
            'import json,sys\nfrom pathlib import Path\n'
            'args=sys.argv[1:]\n'
            'assert args[:5]==["exec","--ephemeral","--sandbox","read-only","--skip-git-repo-check"]\n'
            'assert "--model" not in args and "-c" not in args\n'
            'assert "UNTRUSTED DATA" in sys.stdin.read()\n'
            'Path(args[args.index("--output-last-message")+1]).write_text(json.dumps({"entries":[],"reason":"subprocess fixture only"}))\n')
        executable.chmod(0o700)
        # Verify real process transport, not first-execution startup speed.
        # Keep a finite allowance without changing production timeout behavior.
        result = notes.infer(output, timeout=10, executable=str(executable))
        self.assertEqual(notes.validate_response(result, packet)['entries'], [])
        executable.write_text('#!' + sys.executable + '\nimport sys\nsys.stderr.write("transport fixture refusal\\n")\nsys.exit(7)\n')
        with self.assertRaisesRegex(notes.Refusal, 'exit 7'):
            notes.infer(output, timeout=10, executable=str(executable))
        self.assertIn('transport fixture refusal', (output / 'generation.stderr').read_text())

    def test_actual_result_transport_checks_original_input_execution_and_idempotence(self):
        packet = notes.input_packet(self.root, self.base, self.source)
        directory = self.root / 'packet'
        notes.export_packet(directory, packet)
        response = self.model(directory, 1)
        result = self.root / 'result.json'
        result.write_bytes(notes.encode(response))
        execution = self.root / 'execution.json'
        proof = {'exit_code': 0, 'timed_out': False, 'attempts': 1, 'result_exists': True,
                 'result_sha256': notes.digest(result.read_bytes()), 'completed_utc': '2026-10-18T18:23:00+00:00'}
        execution.write_bytes(notes.encode(proof))
        evidence = self.evidence(directory, result, execution)
        self.assertTrue(notes.import_result(self.root, directory, result, execution, *evidence))
        before = (self.root / notes.DATASET).read_bytes()
        self.assertFalse(notes.import_result(self.root, directory, result, execution, *evidence))
        notes.validate_current(self.root, self.source)
        proof['exit_code'] = 1
        execution.write_bytes(notes.encode(proof))
        failed_evidence = self.evidence(directory, result, execution)
        with self.assertRaisesRegex(notes.Refusal, 'execution did not'):
            notes.import_result(self.root, directory, result, execution, *failed_evidence)
        self.assertEqual((self.root / notes.DATASET).read_bytes(), before)
        (directory / 'prompt.txt').write_text('different prompt')
        with self.assertRaisesRegex(notes.Refusal, 'schema or prompt'):
            notes.import_result(self.root, directory, result, execution, *evidence)
        self.assertEqual((self.root / notes.DATASET).read_bytes(), before)

    def test_package_payload_retains_exact_generated_bytes_and_rejects_tampering(self):
        import sys
        spec = importlib.util.spec_from_file_location('changelog_package_fixture', Path(__file__).resolve().parent.parent / 'hex-test-build.py')
        package = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = package
        spec.loader.exec_module(package)
        self.assertEqual(package.changelog_payload(self.root), (None, {'status': 'unavailable', 'coverage': []}))
        notes.prepare(self.root, self.base, model=self.model)
        raw, identity = package.changelog_payload(self.root)
        self.assertEqual(raw, (self.root / notes.DATASET).read_bytes())
        self.assertEqual(identity['sha256'], notes.digest(raw))
        self.assertEqual(identity['coverage'], [self.source])
        self.commit('retain notes')
        (self.root / 'later.txt').write_text('unrelated later candidate content')
        self.commit('later unrelated candidate')
        self.assertEqual(package.changelog_payload(self.root), (raw, identity))
        (self.root / notes.DATASET).write_bytes(raw.replace(b'fixes', b'wrong'))
        with self.assertRaises(package.DeliveryError):
            package.changelog_payload(self.root)

    def test_pipeline_preparation_exits_before_sealed_runner_and_propagates_failure(self):
        import sys
        from types import SimpleNamespace
        from unittest.mock import patch
        spec = importlib.util.spec_from_file_location('changelog_ci_fixture', Path(__file__).resolve().parent.parent / 'local-ci.py')
        pipeline = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = pipeline
        spec.loader.exec_module(pipeline)
        with patch.object(pipeline, 'LocalCiRunner', side_effect=AssertionError('sealed runner started during preparation')), patch.object(pipeline.subprocess, 'run', return_value=SimpleNamespace(returncode=7)) as command:
            self.assertEqual(pipeline.main(['--prepare-changelog', '--changelog-base', self.base, '--changelog-through', self.source]), 7)
            args = command.call_args.args[0]
            self.assertEqual(args[-4:], ['--source', self.source, '--base', self.base])

    def test_broken_dataset_and_redirected_output_directory_are_refused(self):
        path = self.root / notes.DATASET
        path.parent.mkdir()
        path.symlink_to(self.root / 'missing.json')
        with self.assertRaisesRegex(notes.Refusal, 'bounded regular file'):
            notes.load_dataset(path)
        path.unlink()
        path.parent.rmdir()
        foreign = self.root / 'foreign'
        foreign.mkdir()
        path.parent.symlink_to(foreign, target_is_directory=True)
        with self.assertRaisesRegex(notes.Refusal, 'directory is a symlink'):
            notes.atomic_dataset(path, {'format': 1, 'batches': []})
        self.assertEqual(list(foreign.iterdir()), [])

    def test_pre311_entrypoints_start_without_toml_modules(self):
        import sys
        root = Path(__file__).resolve().parents[2]
        harness = """import builtins,runpy,sys
original=builtins.__import__
def refuse(name,*args,**kwargs):
    if name in ('tomllib','tomli'):
        raise ModuleNotFoundError('unavailable before Python 3.11',name=name)
    return original(name,*args,**kwargs)
builtins.__import__=refuse
entry=sys.argv[1]
sys.argv=[entry,'--help']
runpy.run_path(entry,run_name='__main__')
"""
        for entry in [root / 'scripts/hex-test-build.py', root / 'scripts/changelog.py']:
            result = subprocess.run([sys.executable, '-c', harness, str(entry)], text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('usage:', result.stdout)

    def test_sibling_coverage_is_not_candidate_history(self):
        self.git('checkout', '-qb', 'sibling', self.base)
        (self.root / 'feature.txt').write_text('a different sibling feature\n')
        sibling = self.commit('different sibling feature')
        def model(directory, timeout):
            return {'entries': [{'category': 'features', 'text': 'a sibling-only fixture feature.', 'commits': [sibling]}], 'reason': 'fixture only'}
        notes.prepare(self.root, self.base, model=model)
        raw = (self.root / notes.DATASET).read_bytes()
        self.git('checkout', '-q', self.source)
        with self.assertRaisesRegex(notes.Refusal, 'ancestor'):
            notes.validate_current(self.root, if_present=True)
        import sys
        spec = importlib.util.spec_from_file_location('sibling_package_fixture', Path(__file__).resolve().parent.parent / 'hex-test-build.py')
        package = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = package
        spec.loader.exec_module(package)
        with self.assertRaisesRegex(package.DeliveryError, 'ancestor'):
            package.changelog_payload(self.root)
        self.assertEqual(self.git('rev-parse', 'HEAD'), self.source)
        self.assertEqual((self.root / notes.DATASET).read_bytes(), raw)

    def test_prepare_nonancestor_refuses_before_model_or_write(self):
        notes.prepare(self.root, self.base, model=self.model)
        candidate = self.commit('retain valid notes')
        path = self.root / notes.DATASET
        original = path.read_bytes()
        self.git('checkout', '-qb', 'sibling-prepare', self.base)
        (self.root / 'feature.txt').write_text('sibling-only change\n')
        sibling = self.commit('sibling-only feature')
        self.git('checkout', '-q', candidate)
        calls = []
        def model(directory, timeout):
            calls.append(directory)
            return {'entries': [{'category': 'features', 'text': 'a sibling-only feature.', 'commits': [sibling]}], 'reason': 'injected fixture, not inference'}
        try:
            with self.assertRaisesRegex(notes.Refusal, 'ancestor'):
                notes.prepare(self.root, self.base, source=sibling, model=model)
        finally:
            self.assertEqual(calls, [], 'nonancestor preparation must not invoke the model')
            self.assertEqual(path.read_bytes(), original)
            self.assertEqual(self.git('rev-parse', 'HEAD'), candidate)

    def test_followup_export_retains_history_for_second_import(self):
        notes.prepare(self.root, self.base, model=self.model)
        self.commit('retain first notes')
        previous = ['clip selection stays consistent.']
        (self.root / 'feature.txt').write_text('third change\n')
        current = self.commit('second user feature')
        directory = self.root / 'second-packet'
        self.assertEqual(notes.main(['export', '--root', str(self.root), '--base', self.source,
                                     '--source', current, '--output', str(directory)]), 0)
        packet = notes.input_packet(self.root, self.source, current)
        self.assertEqual((directory / 'prompt.txt').read_bytes(), notes.prompt(packet, previous).encode())
        result = self.root / 'second-result.json'
        result.write_bytes(notes.encode({'entries': [{'category': 'features', 'text': 'a second user feature.', 'commits': [current]}], 'reason': 'injected fixture only'}))
        execution = self.root / 'second-execution.json'
        execution.write_bytes(notes.encode({'exit_code': 0, 'timed_out': False, 'attempts': 1, 'result_exists': True,
            'result_sha256': notes.digest(result.read_bytes()), 'completed_utc': '2026-10-18T18:23:00+00:00'}))
        evidence = self.evidence(directory, result, execution)
        self.assertTrue(notes.import_result(self.root, directory, result, execution, *evidence))
        self.assertEqual(len(notes.validate_current(self.root)['batches']), 2)
        repeated = self.root / 'repeated-second-packet'
        self.assertEqual(notes.main(['export', '--root', str(self.root), '--base', self.source,
                                     '--source', current, '--output', str(repeated)]), 0)
        self.assertEqual((repeated / 'prompt.txt').read_bytes(), (directory / 'prompt.txt').read_bytes())

    def test_import_malformed_completion_preserves_existing_dataset(self):
        notes.prepare(self.root, self.base, model=self.model)
        self.commit('retained first notes')
        old = (self.root / notes.DATASET).read_bytes()
        (self.root / 'feature.txt').write_text('next feature\n')
        current = self.commit('next feature')
        directory = self.root / 'timestamp-packet'
        notes.export_packet(directory, notes.input_packet(self.root, self.source, current), ['clip selection stays consistent.'])
        result = self.root / 'result.json'
        result.write_bytes(notes.encode({'entries': [], 'reason': 'injected internal-only fixture'}))
        execution = self.root / 'execution.json'
        for timestamp in [None, 123, 'yesterday', '2026-02-30T18:23:00+00:00', '2026-10-18T18:23:00', '2026-10-18T18:23:00+02:00']:
            with self.subTest(timestamp=timestamp):
                proof = {'exit_code': 0, 'timed_out': False, 'attempts': 1, 'result_exists': True,
                         'result_sha256': notes.digest(result.read_bytes()), 'completed_utc': timestamp}
                execution.write_bytes(notes.encode(proof))
                evidence = self.evidence(directory, result, execution)
                (self.root / notes.DATASET).write_bytes(old)
                try:
                    with self.assertRaisesRegex(notes.Refusal, 'completion timestamp'):
                        notes.import_result(self.root, directory, result, execution, *evidence)
                finally:
                    self.assertEqual((self.root / notes.DATASET).read_bytes(), old)

    def test_success_removes_own_private_packet_but_failure_retains_diagnostics(self):
        directories = []
        def success(directory, timeout):
            directories.append(directory)
            return self.model(directory, timeout)
        notes.prepare(self.root, self.base, model=success)
        self.assertFalse(directories[0].parent.exists(), 'successful private source packet must be removed')
        self.commit('retain notes')
        old = (self.root / notes.DATASET).read_bytes()
        (self.root / 'feature.txt').write_text('later feature\n')
        self.commit('later feature')
        def failure(directory, timeout):
            directories.append(directory)
            raise notes.Refusal('injected failure')
        with self.assertRaisesRegex(notes.Refusal, 'diagnostics:'):
            notes.prepare(self.root, model=failure)
        self.assertTrue((directories[-1] / 'input.json').is_file())
        self.assertEqual((self.root / notes.DATASET).read_bytes(), old)
        self.addCleanup(__import__('shutil').rmtree, directories[-1].parent)

    def test_export_is_owner_private_under_public_umask_and_preserves_foreign_directory(self):
        import os
        directory = self.root / 'private-packet'
        old_umask = os.umask(0o022)
        try:
            notes.export_packet(directory, notes.input_packet(self.root, self.base, self.source))
        finally:
            os.umask(old_umask)
        self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
        for path in directory.iterdir():
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        foreign = self.root / 'foreign-packet'
        foreign.mkdir(mode=0o755)
        (foreign / 'user.txt').write_text('keep me')
        mode = foreign.stat().st_mode
        with self.assertRaises(FileExistsError):
            notes.export_packet(foreign, notes.input_packet(self.root, self.base, self.source))
        self.assertEqual(foreign.stat().st_mode, mode)
        self.assertEqual((foreign / 'user.txt').read_text(), 'keep me')

    def test_diff_input_is_independent_of_local_formatting_configuration(self):
        content = 'index ' + 'a' * 40 + '..' + 'b' * 40 + ' 100644'
        (self.root / 'café.txt').write_text(content + '\n')
        current = self.commit('add unicode-path fixture')
        original = notes.input_packet(self.root, self.base, current)
        self.assertIn('+' + content, original['diff'], 'source hunk content must never be abbreviated')
        for key, value in [('diff.noprefix', 'true'), ('diff.mnemonicPrefix', 'true'),
                           ('diff.algorithm', 'histogram'), ('diff.renames', 'false'),
                           ('diff.context', '8'), ('diff.interHunkContext', '5'),
                           ('core.abbrev', '12'), ('core.quotePath', 'false')]:
            self.git('config', key, value)
        self.assertEqual(notes.input_packet(self.root, self.base, current), original)

    def test_nonexistent_evidence_commit_cannot_publish_imported_notes(self):
        directory = self.root / 'proof-packet'
        notes.export_packet(directory, notes.input_packet(self.root, self.base, self.source))
        result = self.root / 'result.json'
        result.write_bytes(notes.encode(self.model(directory, 1)))
        execution = self.root / 'execution.json'
        execution.write_bytes(notes.encode({'exit_code': 0, 'timed_out': False, 'attempts': 1, 'result_exists': True,
            'result_sha256': notes.digest(result.read_bytes()), 'completed_utc': '2026-10-18T18:23:00+00:00'}))
        with self.assertRaisesRegex(notes.Refusal, 'evidence'):
            notes.import_result(self.root, directory, result, execution, 'f' * 40)
        self.assertFalse((self.root / notes.DATASET).exists())

    def test_import_binds_exact_offline_archive_and_preserves_notes_on_wrong_proof(self):
        notes.prepare(self.root, self.base, model=self.model)
        self.commit('retain valid notes')
        old = (self.root / notes.DATASET).read_bytes()
        (self.root / 'feature.txt').write_text('new feature\n')
        current = self.commit('next feature')
        directory = self.root / 'bound-packet'
        notes.export_packet(directory, notes.input_packet(self.root, self.source, current), ['clip selection stays consistent.'])
        result = self.root / 'result.json'
        result.write_bytes(notes.encode({'entries': [], 'reason': 'injected fixture'}))
        execution = self.root / 'execution.json'
        execution.write_bytes(notes.encode({'exit_code': 0, 'timed_out': False, 'attempts': 1, 'result_exists': True,
            'result_sha256': notes.digest(result.read_bytes()), 'completed_utc': '2026-10-18T18:23:00Z'}))
        for overrides in [{'result.json': b'other result'}, {'execution.json': b'other report'},
                          {'original-input/input.json': b'other input'}, {'../escape': b'preserve outside files'}]:
            with self.subTest(overrides=overrides):
                evidence = self.evidence(directory, result, execution, overrides)
                with self.assertRaisesRegex(notes.Refusal, 'evidence'):
                    notes.import_result(self.root, directory, result, execution, *evidence)
                self.assertEqual((self.root / notes.DATASET).read_bytes(), old)
        for options in [{'corrupt_manifest': True}, {'link': True}, {'duplicate': True}, {'unmanifested': True}]:
            with self.subTest(options=options):
                evidence = self.evidence(directory, result, execution, **options)
                with self.assertRaisesRegex(notes.Refusal, 'evidence'):
                    notes.import_result(self.root, directory, result, execution, *evidence)
                self.assertEqual((self.root / notes.DATASET).read_bytes(), old)
        evidence = self.evidence(directory, result, execution)
        self.assertTrue(notes.import_result(self.root, directory, result, execution, *evidence))
        self.assertEqual(len(notes.validate_current(self.root)['batches']), 2)

    def test_literal_workspace_version_scope_and_explicit_refusal(self):
        for raw, expected in [('[workspace.package]\nversion = "0.1.0"\n', '0.1.0'),
                              ("[workspace.package] # workspace metadata\nversion = '1.2.3-beta.1+build' # bounded literal\n", '1.2.3-beta.1+build'),
                              ('[package]\nversion="9.9.9"\n[workspace.package]\nversion="0.1.0"\n[workspace.dependencies]\nversion="8.8.8"\n', '0.1.0')]:
            self.assertEqual(notes.workspace_version(raw), expected)
        for raw in ['[package]\nversion="0.1.0"\n', '[workspace.package]\n',
                    '[workspace.package]\nversion.workspace=true\n',
                    '[workspace.package]\nversion=123\n',
                    '[workspace.package]\nversion="0.1.0"\nversion="0.2.0"\n',
                    '[workspace.package]\nversion="0.1.0"\n[workspace.package]\nversion="0.2.0"\n']:
            with self.assertRaises(notes.Refusal):
                notes.workspace_version(raw)


    def test_explicit_followup_base_cannot_skip_or_overlap_covered_history(self):
        notes.prepare(self.root, self.base, model=self.model)
        self.commit('retain first notes')
        (self.root / 'feature.txt').write_text('middle change\n')
        middle = self.commit('middle change')
        (self.root / 'feature.txt').write_text('last change\n')
        current = self.commit('last change')
        before = (self.root / notes.DATASET).read_bytes()
        calls = []
        def model(*_):
            calls.append(True)
            return {'entries': [], 'reason': 'injected fixture'}
        for base in (middle, self.base):
            with self.subTest(base=base):
                with self.assertRaisesRegex(notes.Refusal, 'continuation'):
                    notes.prepare(self.root, base, source=current, model=model)
                self.assertEqual(calls, [])
                self.assertEqual((self.root / notes.DATASET).read_bytes(), before)

    def test_history_independently_refuses_a_gap_or_overlap(self):
        notes.prepare(self.root, self.base, model=self.model)
        self.commit('first notes')
        (self.root / 'feature.txt').write_text('middle\n')
        middle = self.commit('middle')
        (self.root / 'feature.txt').write_text('last\n')
        current = self.commit('last')
        dataset = notes.load_dataset(self.root / notes.DATASET)
        previous = notes.validate_history(self.root, dataset)
        for base in (middle, self.base):
            packet = notes.input_packet(self.root, base, current)
            batch = {'input': notes.packet_identity(packet, previous), 'input_sha256': notes.digest(notes.encode(packet)),
                     'generated_at': '2026-10-18T20:00:00Z', 'generator': 'configured codex cli',
                     'result': {'entries': [], 'reason': 'injected fixture'}}
            with self.subTest(base=base), self.assertRaisesRegex(notes.Refusal, 'continuation'):
                notes.validate_history(self.root, {'format': 1, 'batches': dataset['batches'] + [batch]})

    def test_git_diff_is_capped_during_read_not_after_communicate(self):
        from unittest.mock import patch
        original = subprocess.Popen
        class BoundedDiff(original):
            def communicate(self, *args, **kwargs):
                if 'diff' in self.args:
                    raise AssertionError('unbounded diff communicate before input limit')
                return super().communicate(*args, **kwargs)
        (self.root / 'large.txt').write_text('large source line\n' * 5000)
        current = self.commit('large diff fixture')
        with patch.object(notes, 'MAX_INPUT', 2048), patch.object(notes.subprocess, 'Popen', BoundedDiff):
            with self.assertRaisesRegex(notes.Refusal, 'limit|512 KiB'):
                notes.input_packet(self.root, self.base, current)

    def test_import_requires_integer_exit_and_attempts_not_boolean_aliases(self):
        directory = self.root / 'packet'
        notes.export_packet(directory, notes.input_packet(self.root, self.base, self.source))
        result = self.root / 'result.json'
        result.write_bytes(notes.encode(self.model(directory, 120)))
        execution = self.root / 'execution.json'
        for exit_code, attempts in ((False, 1), (0, True)):
            execution.write_bytes(notes.encode({'exit_code': exit_code, 'timed_out': False, 'attempts': attempts,
                'result_exists': True, 'result_sha256': notes.digest(result.read_bytes()), 'completed_utc': '2026-10-18T20:00:00Z'}))
            evidence = self.evidence(directory, result, execution)
            with self.subTest(exit_code=exit_code, attempts=attempts), self.assertRaisesRegex(notes.Refusal, 'execution did not'):
                notes.import_result(self.root, directory, result, execution, *evidence)
            self.assertFalse((self.root / notes.DATASET).exists())

    def test_successful_concurrent_publisher_cannot_be_overwritten_by_stale_model(self):
        winner = []
        def slow_model(directory, timeout):
            # Exact production cutpoint: another invocation publishes while the
            # first model is running, after both captured the same initial bytes.
            notes.prepare(self.root, self.base, model=self.model)
            winner.append((self.root / notes.DATASET).read_bytes())
            result = self.model(directory, timeout)
            result['entries'][0]['text'] = 'a stale second result must not replace successful notes.'
            return result
        with self.assertRaisesRegex(notes.Refusal, 'changed|retry'):
            notes.prepare(self.root, self.base, model=slow_model)
        self.assertEqual((self.root / notes.DATASET).read_bytes(), winner[0])
        notes.validate_current(self.root)


    def test_publication_lock_contention_preserves_successful_notes_and_source(self):
        import fcntl
        notes.prepare(self.root, self.base, model=self.model)
        before = (self.root / notes.DATASET).read_bytes()
        self.commit('retained notes')
        (self.root / 'feature.txt').write_text('next feature\n')
        current = self.commit('next feature')
        identity = notes.digest(__import__('os').fsencode((self.root / notes.DATASET).resolve()))
        lock_path = Path(self.git('rev-parse', '--git-path', f'hex-changelog-{identity}.lock'))
        if not lock_path.is_absolute():
            lock_path = self.root / lock_path
        with lock_path.open('rb') as lock:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaisesRegex(notes.Refusal, 'publisher is active'):
                notes.prepare(self.root, model=lambda *_: {'entries': [], 'reason': 'fixture only'})
        self.assertEqual((self.root / notes.DATASET).read_bytes(), before)
        self.assertEqual(self.git('rev-parse', 'HEAD'), current)
        self.assertEqual(self.git('status', '--porcelain'), '')


    def test_real_multiprocess_publishers_through_root_alias_preserve_first_success(self):
        (self.root / 'feature.txt').write_text('a later independently requested batch\n')
        later_source = self.commit('later range for competing publisher')
        import sys
        import time
        with tempfile.TemporaryDirectory(prefix='hex-changelog-contenders-') as raw:
            outside = Path(raw)
            alias = outside / 'same-root'
            alias.symlink_to(self.root, target_is_directory=True)
            script = outside / 'worker.py'
            script.write_text('''import importlib.util,sys,time
from pathlib import Path
spec=importlib.util.spec_from_file_location('notes',sys.argv[1]);notes=importlib.util.module_from_spec(spec);spec.loader.exec_module(notes)
root,base,source,gate,text=Path(sys.argv[2]),sys.argv[3],sys.argv[4],Path(sys.argv[5]),sys.argv[6]
def model(*_):
 gate.with_suffix('.ready').touch()
 end=time.monotonic()+10
 while not gate.exists():
  if time.monotonic()>end: raise RuntimeError('fixture gate timeout')
  time.sleep(.01)
 return {'entries':[{'category':'fixes','text':text,'commits':[source]}],'reason':'injected subprocess fixture, no model call'}
try: notes.prepare(root,base,source=source,model=model)
except notes.Refusal as error: print(str(error));sys.exit(3)
''')
            workers = []
            try:
                for index, root in enumerate((self.root, alias)):
                    gate = outside / f'gate{index}'
                    process = subprocess.Popen([sys.executable, str(script), str(Path(notes.__file__).resolve()), str(root), self.base, (self.source, later_source)[index], str(gate), f'publisher {index} fixture.'], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                    workers.append((process, gate))
                end = time.monotonic() + 10
                while not all(gate.with_suffix('.ready').exists() for _, gate in workers):
                    self.assertLess(time.monotonic(), end, 'both real processes must reach the same pre-publication cutpoint')
                    time.sleep(.01)
                workers[0][1].touch()
                first, errors = workers[0][0].communicate(timeout=10)
                self.assertEqual(workers[0][0].returncode, 0, errors)
                retained = (self.root / notes.DATASET).read_bytes()
                workers[1][1].touch()
                last, errors = workers[1][0].communicate(timeout=10)
                self.assertEqual(workers[1][0].returncode, 3, errors)
                self.assertIn(b'changelog changed', last)
                self.assertEqual((self.root / notes.DATASET).read_bytes(), retained)
                self.assertIn(b'publisher 0 fixture.', retained)
                self.assertEqual(len(notes.validate_current(self.root)['batches']), 1)
                self.assertEqual(notes.load_dataset(self.root / notes.DATASET)['batches'][0]['input']['source'], self.source)
            finally:
                for process, _ in workers:
                    if process.poll() is None:
                        process.kill()
                    process.communicate(timeout=10)

    def test_git_pipe_overflow_failure_and_interrupt_reap_child_with_bounded_stderr(self):
        import sys
        from unittest.mock import patch
        children = []
        original = subprocess.Popen
        def child(argv, **options):
            behavior = argv[-1]
            program = {'overflow': "import os;os.write(2,b'e'*200000);os.write(1,b'x'*200000)",
                       'failure': "import os;os.write(2,b'e'*200000);raise SystemExit(7)",
                       'interrupt': 'import time;time.sleep(10)'}[behavior]
            process = original([sys.executable, '-c', program], **options)
            children.append(process)
            return process
        with patch.object(notes.subprocess, 'Popen', child):
            with self.assertRaisesRegex(notes.Refusal, 'limit'):
                notes.git(self.root, 'overflow', max_bytes=1024)
            self.assertIsNotNone(children[-1].poll())
            with self.assertRaisesRegex(notes.Refusal, 'exit 7') as refusal:
                notes.git(self.root, 'failure', max_bytes=1024)
            self.assertLess(len(str(refusal.exception)), 8300)
            self.assertEqual(children[-1].returncode, 7)
            with patch.object(notes.selectors.DefaultSelector, 'select', side_effect=KeyboardInterrupt):
                with self.assertRaises(KeyboardInterrupt):
                    notes.git(self.root, 'interrupt', max_bytes=1024)
            self.assertIsNotNone(children[-1].poll())
            for process in children:
                self.assertTrue(process.stdout.closed and process.stderr.closed)


    def test_export_and_import_cannot_append_a_gap_despite_valid_result_evidence(self):
        notes.prepare(self.root, self.base, model=self.model)
        self.commit('first notes')
        before = (self.root / notes.DATASET).read_bytes()
        previous = ['clip selection stays consistent.']
        (self.root / 'feature.txt').write_text('middle\n')
        middle = self.commit('middle')
        (self.root / 'feature.txt').write_text('last\n')
        current = self.commit('last')
        for index, base in enumerate((middle, self.base)):
            directory = self.root / f'gap-packet-{index}'
            self.assertEqual(notes.main(['export', '--root', str(self.root), '--base', base,
                '--source', current, '--output', str(directory)]), 1)
            self.assertFalse(directory.exists())
            packet = notes.input_packet(self.root, base, current)
            notes.export_packet(directory, packet, previous)
            result = self.root / 'gap-result.json'
            result.write_bytes(notes.encode({'entries': [], 'reason': 'injected empty classification'}))
            execution = self.root / 'gap-execution.json'
            execution.write_bytes(notes.encode({'exit_code': 0, 'timed_out': False, 'attempts': 1, 'result_exists': True,
                'result_sha256': notes.digest(result.read_bytes()), 'completed_utc': '2026-10-18T20:00:00Z'}))
            evidence = self.evidence(directory, result, execution)
            with self.assertRaisesRegex(notes.Refusal, 'continuation'):
                notes.import_result(self.root, directory, result, execution, *evidence)
            self.assertEqual((self.root / notes.DATASET).read_bytes(), before)


if __name__ == '__main__':
    unittest.main()
