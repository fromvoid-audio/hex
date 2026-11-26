"""Correlation of producer-stamped Hex VST3 records for serial insertions."""
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from vst3_insertion_timings import attempts


def ui(stage, name='Thermal'):
    return f'vst3_load v=1 stage={stage} class="{name}" generation=1 revision=4 rate_hz=48000 destination=Track(TrackId(3))'


def native(stage, details='', instance=3, graph=5):
    return (f'vst3_insert v=1 stage={stage} generation=1 binding=ForeignProcessorBinding '
            f'{{ root: 2, instance: {instance}, graph: {graph}, epoch: 1 }}{details}')


def records():
    return [
        ui('submitted'), ui('loading_feedback_painted'),
        native('native_prepare_returned', ' elapsed_ns=2000000 selection_inspection_ns=40000 revalidation_ns=0 waitable_probe_ns=None instance_to_facts_ns=Some(1800000)'),
        native('native_facts'), native('endpoint_ready'), native('graph_terminal', ' committed=true'),
        native('automatic_editor_queued'), ui('accepted'), native('automatic_editor_terminal', ' open=true'),
    ]


class InsertionTimingsTests(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.folder = Path(folder.name)

    def segment(self, messages, segment=0, first=1, session='capture'):
        path = self.folder / f'{session}-{segment}.log'
        lines = [f'session_started log_schema=3 session={session} segment={segment} context_truncated=false context="source=abc:clean"\n']
        for sequence, message in enumerate(messages, first):
            if message is None:
                continue
            category = 'ui' if message.startswith('vst3_load') else 'audio'
            lines.append(f'event level=debug category={category} elapsed_nanos={sequence * 1000000} sequence={sequence} truncated=false message={json.dumps(message)}\n')
        path.write_text(''.join(lines), encoding='utf-8')
        return path

    def test_rotated_serial_attempts_keep_source_binding_and_stage_times(self):
        data = records()
        a = self.segment(data[:4])
        b = self.segment(data[4:] + records(), 1, 5)
        first, second = attempts([b, a])
        self.assertEqual((first['status'], second['status']), ('complete', 'complete'))
        self.assertEqual((first['source'], first['class'], first['instance'], first['graph']),
                         ('abc:clean', 'Thermal', '3', '5'))
        self.assertEqual((first['paint_ms'], first['facts_ms'], first['installed_ms'], first['editor_reply_ms']),
                         ('1.000000', '3.000000', '5.000000', '8.000000'))
        self.assertEqual(first['instance_to_facts_ms'], '1.800000')
        self.assertNotIn('waitable_probe_ms', first)
        self.assertEqual(second['sequence'], 10)

    def test_partial_refused_and_ambiguous_records_are_visible(self):
        tests = [
            (records()[:-1], 'incomplete', 'editor_reply'),
            (records()[:-1] + [native('automatic_editor_terminal', ' open=false')], 'refused', 'editor refused'),
            ([ui('submitted'), ui('refused')], 'refused', 'UI refused'),
            (records()[:4] + [native('endpoint_ready', instance=9)] + records()[5:], 'incomplete', 'ambiguous native binding'),
            (records()[:4] + [native('endpoint_ready', graph=9)] + records()[5:], 'incomplete', 'endpoint'),
            (records()[:4] + [native('native_facts')] + records()[4:], 'incomplete', 'duplicate'),
            (records()[:3] + [None] + records()[3:], 'incomplete', 'discontinuity'),
        ]
        for data, status, detail in tests:
            with self.subTest(detail=detail):
                row, = attempts([self.segment(data)])
                self.assertEqual(row['status'], status)
                self.assertIn(detail, row['reason'])

    def test_overlapping_successor_revision_is_ambiguous_across_classes(self):
        a, b = attempts([self.segment([ui('submitted', 'Other')] + records())])
        self.assertIn('next submission', a['reason'])
        self.assertIn('ambiguous repeated submission', b['reason'])

    def test_mixed_sessions_and_duplicate_segments_are_rejected(self):
        a = self.segment(records())
        b = self.segment(records(), session='other')
        for files in ([a, b], [a, a]):
            with self.subTest(files=files), self.assertRaises(ValueError):
                list(attempts(files))


if __name__ == '__main__':
    unittest.main()
