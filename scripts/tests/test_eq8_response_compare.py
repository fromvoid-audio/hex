"""Synthetic comparator controls: not a Live/reference dataset."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import subprocess
import sys
import unittest

SPEC = importlib.util.spec_from_file_location('eq8_compare', Path(__file__).parents[1] / 'eq8-response-compare.py')
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ComparisonTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.case = dict(sample_rate=48000, mode='lr', bank='left', shape='bell',
                         frequency_hz=1000, gain_db=0, q=0.70710677, output_gain=1, adaptive_q=False)
        self.capture = b'query_hz,lane0_linear,lane1_linear\n100,1,1\n1000,1,1\n'
        self.manifest = dict(schema=1, kind='synthetic', case=self.case,
                             provenance=dict(producer='synthetic controls only', version='fixture-1',
                                             executable_sha256='a' * 64, method='arithmetic test values, no native measurement',
                                             capture=dict(path='capture.csv', sha256=hashlib.sha256(self.capture).hexdigest())),
                             uncertainty=dict(absolute_linear=0.02, relative_linear=0),
                             tolerance=dict(absolute_linear=0.05, relative_linear=0))
        self.metadata = dict(schema=1, producer='hex.eq8 prepared response', state_version=1,
                             source_sha='b' * 40, dirty=False, case=self.case)

    def write(self, value=1):
        (self.root / 'capture.csv').write_bytes(self.capture)
        (self.root / 'manifest.json').write_text(json.dumps(self.manifest))
        (self.root / 'hex.csv').write_text('# ' + json.dumps(self.metadata) +
                                         f'\nquery_hz,lane0_linear,lane1_linear\n100,{value},1\n1000,1,1\n')
        return self.root / 'manifest.json', self.root / 'hex.csv'

    def test_uncertainty_has_three_distinct_outcomes(self):
        for value, expected in [(1.02, 'within_tolerance'), (1.04, 'inconclusive'), (1.08, 'outside_tolerance')]:
            report = MODULE.compare(*self.write(value))
            self.assertEqual(report['status'], expected)
            self.assertFalse(report['parity_claim'])
            self.assertEqual(report['kind'], 'synthetic')

    def test_balanced_tolerance_does_not_hide_wrong_lane(self):
        paths = self.write()
        paths[1].write_text(paths[1].read_text().replace('100,1,1', '100,1,2'))
        report = MODULE.compare(*paths)
        self.assertEqual(report['status'], 'outside_tolerance')
        self.assertEqual(report['points'][1]['lane'], 1)

    def test_no_interpolation_or_missing_points(self):
        for source, target in [('1000,1,1', '1001,1,1'), ('1000,1,1\n', '')]:
            paths = self.write()
            paths[1].write_text(paths[1].read_text().replace(source, target))
            with self.assertRaises(MODULE.DatasetError):
                MODULE.compare(*paths)

    def test_hash_and_missing_capture_refuse(self):
        paths = self.write()
        (self.root / 'capture.csv').write_bytes(self.capture + b'corrupt')
        with self.assertRaisesRegex(MODULE.DatasetError, 'SHA256 mismatch'):
            MODULE.compare(*paths)
        (self.root / 'capture.csv').unlink()
        with self.assertRaisesRegex(MODULE.DatasetError, 'missing/unreadable'):
            MODULE.compare(*paths)

    def test_unknown_duplicate_and_nonfinite_json(self):
        paths = self.write()
        for content in ['{"schema":1,"schema":1}', '{"schema":NaN}', '{"schema":1,"extra":0}',
                        '[' * 10000 + '0' + ']' * 10000]:
            paths[0].write_text(content)
            with self.assertRaises(MODULE.DatasetError):
                MODULE.compare(*paths)

    def test_nonfinite_negative_duplicate_and_nyquist_csv(self):
        for row in ['100,NaN,1', '100,-1,1', '24000,1,1', '100,1,1\n100,1,1']:
            paths = self.write()
            (self.root / 'capture.csv').write_text('query_hz,lane0_linear,lane1_linear\n' + row + '\n')
            self.manifest['provenance']['capture']['sha256'] = hashlib.sha256((self.root / 'capture.csv').read_bytes()).hexdigest()
            paths[0].write_text(json.dumps(self.manifest))
            with self.assertRaises(MODULE.DatasetError):
                MODULE.compare(*paths)

    def test_adaptive_on_is_not_compared_to_legacy(self):
        self.manifest['case'] = dict(self.case, adaptive_q=True)
        with self.assertRaisesRegex(MODULE.DatasetError, 'adaptive-on'):
            MODULE.compare(*self.write())

    def test_changed_authored_case_and_dirty_source_refuse(self):
        self.metadata['case'] = dict(self.case, gain_db=6)
        with self.assertRaisesRegex(MODULE.DatasetError, 'case mismatch'):
            MODULE.compare(*self.write())
        self.metadata['case'] = self.case
        self.metadata['dirty'] = True
        with self.assertRaisesRegex(MODULE.DatasetError, 'dirty Hex'):
            MODULE.compare(*self.write())

    def test_capture_cannot_escape_fixture_package(self):
        for path in ['/tmp/external.csv', '../external.csv']:
            self.manifest['provenance']['capture']['path'] = path
            with self.assertRaisesRegex(MODULE.DatasetError, 'inside manifest'):
                MODULE.compare(*self.write())

    def test_zero_magnitude_is_finite_without_db_floor(self):
        self.capture = b'query_hz,lane0_linear,lane1_linear\n100,0,1\n1000,1,1\n'
        self.manifest['provenance']['capture']['sha256'] = hashlib.sha256(self.capture).hexdigest()
        self.assertEqual(MODULE.compare(*self.write(0))['status'], 'within_tolerance')

    def test_future_schema_bad_provenance_and_uncertainty_refuse(self):
        original = copy.deepcopy(self.manifest)
        for key, value in [('schema', 2), ('provenance', {}), ('uncertainty', dict(absolute_linear=-1, relative_linear=0))]:
            self.manifest = copy.deepcopy(original)
            self.manifest[key] = value
            with self.assertRaises(MODULE.DatasetError):
                MODULE.compare(*self.write())

    def test_cli_statuses_and_refusal_are_observable(self):
        for value, expected in [(1.02, 0), (1.04, 3), (1.08, 1)]:
            paths = self.write(value)
            result = subprocess.run([sys.executable, str(SPEC.origin), *map(str, paths)], capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, expected, result.stderr)
            self.assertFalse(json.loads(result.stdout)['parity_claim'])
        paths[0].unlink()
        result = subprocess.run([sys.executable, str(SPEC.origin), *map(str, paths)], capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 2)
        self.assertIn('missing/unreadable', result.stderr)
        paths = self.write()
        paths[0].write_text('[' * 10000 + '0' + ']' * 10000)
        result = subprocess.run([sys.executable, str(SPEC.origin), *map(str, paths)], capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 2)
        self.assertIn('comparison refused:', result.stderr)
        self.assertNotIn('Traceback', result.stderr)

    def test_size_and_point_bounds_are_enforced(self):
        path = self.root / 'oversized'
        path.write_bytes(b'x' * (MODULE.MAX_BYTES + 1))
        with self.assertRaisesRegex(MODULE.DatasetError, 'exceeds'):
            MODULE.read(path)
        data = ('query_hz,lane0_linear,lane1_linear\n' + ''.join(f'{i},1,1\n' for i in range(1, MODULE.MAX_POINTS + 2))).encode()
        with self.assertRaisesRegex(MODULE.DatasetError, 'too many'):
            MODULE.samples(data, 192000)

    def test_f32_configuration_equivalence_is_exact(self):
        self.assertTrue(MODULE.equivalent_cases(self.case, dict(self.case, q=0.7071067690849304)))
        self.assertFalse(MODULE.equivalent_cases(self.case, dict(self.case, q=0.7071068)))


if __name__ == '__main__':
    unittest.main()
