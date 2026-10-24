"""Deterministic Git fixtures for the resumable release ledger."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import patch

MODULE = Path(__file__).resolve().parents[1] / 'release_notes.py'
SPEC = importlib.util.spec_from_file_location('release_notes', MODULE)
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class ReleaseLedgerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='hex-release-ledger-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / 'repo'
        self.root.mkdir()
        self.store = Path(self.tmp.name) / 'ledger'
        self.git('init', '-q')
        self.git('config', 'user.name', 'fixture')
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.write('Cargo.toml', '[workspace.package]\nversion = "0.2.0-beta.1"\n')
        self.write('track.txt', 'initial\n')
        self.base = self.commit('baseline')

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args], stderr=subprocess.PIPE).decode().strip()

    def write(self, name, text):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def commit(self, subject, allow_empty=False):
        self.git('add', '-A')
        args = ['-c', 'user.name=fixture', '-c', 'user.email=fixture@example.invalid', 'commit']
        if allow_empty:
            args.append('--allow-empty')
        args.extend(['-qm', subject])
        self.git(*args)
        return self.git('rev-parse', 'HEAD')

    def add_track_change(self, value, subject):
        self.write('track.txt', value + '\n')
        return self.commit(subject)

    @staticmethod
    def response(directory, timeout):
        data = json.loads((directory / 'input.json').read_bytes())
        commit = data['commits'][0]['commit']
        path = data['diff'].splitlines()[0].split(' b/', 1)[-1].strip() if data['diff'] else 'change'
        return {'entries': [{'category': 'features', 'text': f'{path.lower()} is available.',
                             'commits': [commit]}], 'reason': 'injected fixture response; no live inference'}

    def test_first_parent_merge_and_idempotent_missed_catchup(self):
        first = self.add_track_change('one', 'first user change')
        result = release.collect(self.root, self.store, first, self.base)
        self.assertEqual([r['source'] for r in result['records']], [first])
        self.assertEqual(release.collect(self.root, self.store, first, self.base)['records'], result['records'])
        self.git('checkout', '-qb', 'side', self.base)
        self.write('side.txt', 'side change\n')
        side = self.commit('side branch change')
        self.git('checkout', '-q', first)
        self.write('main.txt', 'main change\n')
        main = self.commit('main branch change')
        self.git('merge', '--no-ff', '-qm', 'merge side work', side)
        merged = self.git('rev-parse', 'HEAD')
        caught_up = release.collect(self.root, self.store, merged)
        sources = [record['source'] for record in caught_up['records']]
        self.assertEqual(sources, [first, main, merged])
        self.assertNotIn(side, sources)
        self.assertEqual(len(release.collect(self.root, self.store, merged)['records']), 3)

    def test_internal_empty_change_and_failed_generation_retries_exact_packet(self):
        empty = self.commit('empty bookkeeping commit', allow_empty=True)
        notes = self.add_track_change('second', 'second user change')
        collected = release.collect(self.root, self.store, notes, self.base)
        empty_record = next(record for record in collected['records'] if record['source'] == empty)
        empty_item = next(item for item in next(iter(collected['views'].values()))['items']
                          if item['record_id'] == empty_record['id'])
        self.assertEqual(empty_item['state'], 'internal')
        calls = []
        def fail(directory, timeout):
            calls.append(json.loads((directory / 'input.json').read_bytes()))
            raise ValueError('injected inference failure')
        with self.assertRaisesRegex(release.changelog.Refusal, 'retry reuses exact source input'):
            release.prepare(self.root, self.store, notes, model=fail)
        with self.assertRaisesRegex(release.changelog.Refusal, 'pending or failed chunk'):
            release.snapshot(self.root, self.store, notes, self.base)
        after_failure = release._state(self.store)
        failed_item = next(item for view in after_failure['views'].values() for item in view['items']
                           if item['record_id'] == next(r['id'] for r in after_failure['records'] if r['source'] == notes))
        failed = next(record for record in after_failure['records'] if record['source'] == notes)
        original_hash = failed['input_sha256']
        self.assertEqual(failed_item['state'], 'failed')
        final = release.prepare(self.root, self.store, notes, model=self.response)
        ready = next(record for record in final['records'] if record['source'] == notes)
        view_item = next(item for view in final['views'].values() for item in view['items']
                         if item['record_id'] == ready['id'])
        self.assertEqual(view_item['state'], 'ready')
        self.assertEqual(view_item['input_sha256'], original_hash)
        self.assertEqual(view_item['attempts'], 2)
        self.assertEqual(len(calls), 1)

    def test_commit_identity_preserves_reverts_and_concurrent_collection(self):
        one = self.add_track_change('changed', 'change')
        two = self.add_track_change('initial', 'revert change')
        outcomes = []
        failures = []
        def collect():
            try:
                outcomes.append(release.collect(self.root, self.store, two, self.base))
            except Exception as error:
                failures.append(error)
        workers = [threading.Thread(target=collect) for _ in range(3)]
        for worker in workers: worker.start()
        for worker in workers: worker.join(10)
        self.assertTrue(all(isinstance(error, release.Refusal) and 'operation is active' in str(error) for error in failures))
        self.assertTrue(outcomes)
        self.assertTrue(all(not worker.is_alive() for worker in workers))
        records = outcomes[0]['records']
        self.assertEqual([record['source'] for record in records], [one, two])
        self.assertNotEqual(records[0]['input_sha256'], records[1]['input_sha256'])

    def test_rewritten_source_is_refused_instead_of_deduplicated_by_content(self):
        original = self.add_track_change('same tree', 'original source')
        release.collect(self.root, self.store, original, self.base)
        self.git('checkout', '-qb', 'rewritten', self.base)
        rewritten = self.add_track_change('same tree', 'rewritten source')
        self.assertNotEqual(original, rewritten)
        separated = release.collect(self.root, self.store, rewritten, self.base)
        self.assertEqual(len(separated['views']), 2, 'divergent release branches keep separate first-parent views')
        self.assertEqual(len(separated['records']), 2, 'content-equivalent commits retain their distinct full SHA identities')

    def test_explicit_next_release_baseline_creates_view_without_discarding_prior_view(self):
        first = self.add_track_change('release one', 'first stable release change')
        initial = release.collect(self.root, self.store, first, self.base)
        release.prepare(self.root, self.store, first, self.base, model=self.response)
        next_source = self.add_track_change('release two', 'next stable release change')
        continued = release.collect(self.root, self.store, next_source, first)
        release.prepare(self.root, self.store, next_source, first, model=self.response)
        self.assertEqual(len(continued['views']), 2)
        self.assertEqual(len(continued['records']), 2)
        snapshot = release.snapshot(self.root, self.store, next_source, first, allow_injected=True)
        self.assertEqual(snapshot['base'], first)
        self.assertEqual(snapshot['covered_changes'], [next_source])
        prior = release.snapshot(self.root, self.store, first, self.base, allow_injected=True)
        self.assertEqual(prior['covered_changes'], [first])

    def test_snapshot_is_ui_compatible_and_does_not_rewrite_source_changelog(self):
        source = self.add_track_change('release change', 'release user change')
        before = (self.root / 'changelog' / 'generated.json')
        before.parent.mkdir()
        original = b'{"format":1,"batches":[]}\n'
        before.write_bytes(original)
        release.prepare(self.root, self.store, source, self.base, model=self.response)
        with self.assertRaisesRegex(release.Refusal, 'requires configured codex cli'):
            release.snapshot(self.root, self.store, source, self.base)
        snapshot = release.snapshot(self.root, self.store, source, self.base, allow_injected=True)
        self.assertEqual(snapshot['format'], 1)
        self.assertEqual(snapshot['source'], source)
        self.assertEqual(snapshot['base'], self.base)
        self.assertEqual(snapshot['covered_changes'], [source])
        self.assertEqual(snapshot['batches'][-1]['input']['source'], source)
        self.assertEqual(snapshot['batches'][-1]['result']['entries'][0]['text'], 'track.txt is available.')
        self.assertEqual(before.read_bytes(), original)
        later = self.add_track_change('later', 'later change')
        release.collect(self.root, self.store, later)
        stable = release.snapshot(self.root, self.store, source, self.base, allow_injected=True)
        self.assertEqual(stable['source'], source)
        self.assertEqual(stable['covered_changes'], [source])
        self.assertTrue(any(view['source'] == later for view in release._state(self.store)['views'].values()),
                        'reading a stable snapshot must not rewind ledger state')
        self.assertNotEqual(later, source)

    def test_default_snapshot_preserves_legacy_batch_but_explicit_stable_base_omits_it(self):
        source = self.add_track_change('legacy feature', 'legacy source')
        packet = release.changelog.input_packet(self.root, self.base, source)
        result = self.response(self.packet_directory(packet), 10)
        batch = {'input': release.changelog.packet_identity(packet),
                 'input_sha256': release._sha(release._json(packet)),
                 'generated_at': '2026-10-23T09:00:00+00:00',
                 'generator': 'configured codex cli', 'result': result}
        self.write('changelog/generated.json', json.dumps({'format': 1, 'batches': [batch]}))
        notes_commit = self.commit('retain legacy notes')
        release.collect(self.root, self.store, notes_commit, source)
        default = release.snapshot(self.root, self.store, notes_commit)
        explicit = release.snapshot(self.root, self.store, notes_commit, source)
        self.assertEqual(default['batches'][0]['result']['entries'][0]['text'], 'track.txt is available.')
        self.assertEqual(len(default['batches']), 2)
        self.assertEqual(len(explicit['batches']), 1)

    def packet_directory(self, packet):
        from pathlib import Path
        import tempfile
        parent = Path(tempfile.mkdtemp(prefix='hex-release-model-input-'))
        self.addCleanup(lambda: __import__('shutil').rmtree(parent, ignore_errors=True))
        directory = parent / 'packet'
        release.changelog.export_packet(directory, packet)
        return directory

    def test_each_file_group_is_bounded_and_single_oversize_file_refuses(self):
        self.write('large.txt', 'x' * (release.changelog.MAX_INPUT + 1))
        oversized = self.commit('oversized individual source file')
        with self.assertRaisesRegex(release.changelog.Refusal, 'individual changed file exceeds'):
            release.collect(self.root, self.store, oversized, self.base)

    def test_snapshot_refuses_pending_and_partially_completed_chunks(self):
        payload = 'line-\n' * 52000
        self.write('a-large.txt', payload + 'a\n')
        self.write('b-large.txt', payload + 'b\n')
        source = self.commit('two bounded large file changes')
        collected = release.collect(self.root, self.store, source, self.base)
        records = [record for record in collected['records'] if record['source'] == source]
        self.assertEqual(len(records), 2, 'large input is split into deterministic file groups')
        partial = release.prepare(self.root, self.store, source, self.base, model=self.response, max_changes=1)
        items = next(iter(partial['views'].values()))['items']
        self.assertEqual(sum(item['state'] == 'ready' for item in items), 1)
        with self.assertRaisesRegex(release.Refusal, 'pending or failed chunk'):
            release.snapshot(self.root, self.store, source, self.base, allow_injected=True)
        complete = release.prepare(self.root, self.store, source, self.base, model=self.response)
        self.assertEqual(sum(item['state'] == 'ready' for item in next(iter(complete['views'].values()))['items']), 2)
        self.assertEqual(len(release.snapshot(self.root, self.store, source, self.base, allow_injected=True)['batches'][-1]['result']['entries']), 2)

    def test_snapshot_recomputes_source_and_generation_evidence_hashes(self):
        source = self.add_track_change('tamper test', 'tamper evidence source')
        prepared = release.prepare(self.root, self.store, source, self.base, model=self.response)
        record = prepared['records'][0]
        object_path = release._object_path(self.store, record['input_object'])
        original = object_path.read_bytes()
        object_path.write_bytes(original + b' ')
        with self.assertRaisesRegex(release.Refusal, 'object hash mismatch'):
            release.snapshot(self.root, self.store, source, self.base, allow_injected=True)

    def test_retry_adopts_complete_evidence_after_ledger_publication_failure(self):
        source = self.add_track_change('recover publication', 'recover exact generation')
        release.collect(self.root, self.store, source, self.base)
        calls = []
        def model(directory, timeout):
            calls.append(directory)
            return self.response(directory, timeout)
        atomic = release._atomic
        publications = 0
        def fail_after_evidence(path, raw):
            nonlocal publications
            if path.name == 'ledger.json':
                publications += 1
                if publications == 2:
                    raise OSError('injected crash after evidence publication')
            return atomic(path, raw)
        with patch.object(release, '_atomic', side_effect=fail_after_evidence):
            with self.assertRaisesRegex(OSError, 'after evidence publication'):
                release.prepare(self.root, self.store, source, self.base, model=model)
        self.assertEqual(len(calls), 1)
        recovered = release.prepare(self.root, self.store, source, self.base, model=model)
        self.assertEqual(len(calls), 1, 'retry adopts the exact retained response without another model call')
        view_item = next(item for view in recovered['views'].values() for item in view['items'])
        self.assertEqual(view_item['state'], 'ready')
        self.assertEqual(view_item['generator'], 'injected model')
        self.assertEqual(len(release.snapshot(self.root, self.store, source, self.base, allow_injected=True)['batches'][-1]['result']['entries']), 1)

    def test_large_packet_bounds_only_context_and_retains_full_history(self):
        def history_model(directory, timeout):
            packet = json.loads((directory / 'input.json').read_bytes())
            commit = packet['commits'][0]['commit']
            return {'entries': [{'category': 'features',
                    'text': f'change {index} for {commit[:8]} ' + 'x' * 275,
                    'commits': [commit]} for index in range(32)],
                    'reason': 'fixture history'}

        first = self.add_track_change('first history item', 'first history item')
        second = self.add_track_change('second history item', 'second history item')
        release.prepare(self.root, self.store, first, self.base, model=history_model)
        release.prepare(self.root, self.store, second, self.base, model=history_model)
        self.write('large.txt', 'x' * (492 * 1024) + '\n')
        large = self.commit('large user-visible source change')
        collected = release.collect(self.root, self.store, large, self.base)
        record = next(item for item in collected['records'] if item['source'] == large)
        packet = json.loads(release._get_object(self.store, record['input_object']))
        full_previous = []
        for prior in collected['records']:
            if prior['source'] == large:
                break
            item = next(item for view in collected['views'].values() for item in view['items']
                        if item['record_id'] == prior['id'] and item['state'] == 'ready')
            result = json.loads(release._get_object(self.store, item['result_object']))
            full_previous.extend(entry['text'] for entry in result['entries'])
        self.assertGreater(len(release.changelog.prompt(packet, full_previous).encode()), release.changelog.MAX_INPUT)
        small_packet = release.changelog.input_packet(self.root, self.base, first)
        self.assertEqual(release.generation_prompt(small_packet, full_previous[:1]),
                         release.changelog.prompt(small_packet, full_previous[:1]))

        def large_model(directory, timeout):
            prompt = (directory / 'prompt.txt').read_bytes()
            self.assertLessEqual(len(prompt), release.changelog.MAX_INPUT)
            context = json.loads(prompt.split(b'\n', 1)[1])['previous_context']
            self.assertEqual(context['total'], len(full_previous))
            self.assertGreater(context['omitted_count'], 0)
            self.assertEqual(context['included_recent'], full_previous[-len(context['included_recent']):])
            self.assertEqual(context['full_previous_sha256'], release._sha(release._json(full_previous)))
            self.assertEqual((directory / 'input.json').read_bytes(), release._json(packet))
            commit = packet['commits'][0]['commit']
            return {'entries': [{'category': 'features', 'text': 'large source update is available.',
                                 'commits': [commit]}], 'reason': 'fixture large change'}

        release.prepare(self.root, self.store, large, self.base, model=large_model)
        snapshot = release.snapshot(self.root, self.store, large, self.base, allow_injected=True)
        entries = [entry for batch in snapshot['batches'] for entry in batch['result']['entries']]
        self.assertEqual(len(entries), 65)
        self.assertTrue(any(entry['text'] == 'large source update is available.' for entry in entries))


if __name__ == '__main__':
    unittest.main()
