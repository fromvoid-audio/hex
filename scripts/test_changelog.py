"""Actual Git-range/adapter fixture checks; injected model responses are not live AI evidence."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('changelog', Path(__file__).with_name('changelog.py'))
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


if __name__ == '__main__':
    unittest.main()
