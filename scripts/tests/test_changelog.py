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
        result = notes.infer(output, timeout=2, executable=str(executable))
        self.assertEqual(notes.validate_response(result, packet)['entries'], [])
        executable.write_text('#!' + sys.executable + '\nimport sys\nsys.stderr.write("transport fixture refusal\\n")\nsys.exit(7)\n')
        with self.assertRaisesRegex(notes.Refusal, 'exit 7'):
            notes.infer(output, timeout=2, executable=str(executable))
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
        self.assertTrue(notes.import_result(self.root, directory, result, execution, 'a' * 40))
        before = (self.root / notes.DATASET).read_bytes()
        self.assertFalse(notes.import_result(self.root, directory, result, execution, 'a' * 40))
        notes.validate_current(self.root, self.source)
        proof['exit_code'] = 1
        execution.write_bytes(notes.encode(proof))
        with self.assertRaisesRegex(notes.Refusal, 'execution did not'):
            notes.import_result(self.root, directory, result, execution, 'a' * 40)
        self.assertEqual((self.root / notes.DATASET).read_bytes(), before)
        (directory / 'prompt.txt').write_text('different prompt')
        with self.assertRaisesRegex(notes.Refusal, 'schema or prompt'):
            notes.import_result(self.root, directory, result, execution, 'a' * 40)
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


if __name__ == '__main__':
    unittest.main()
