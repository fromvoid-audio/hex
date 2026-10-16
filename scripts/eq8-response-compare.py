#!/usr/bin/env python3
"""Compare measured/synthetic linear lane magnitudes; never assert reference parity."""
import argparse
import csv
import hashlib
import io
import json
import math
from pathlib import Path
import struct
import sys

MAX_BYTES = 1024 * 1024
MAX_POINTS = 8192
SHAPES = ('bell', 'low_shelf', 'high_shelf', 'low_cut_12', 'low_cut_48',
          'high_cut_12', 'high_cut_48', 'notch')
CASE_KEYS = {'sample_rate', 'mode', 'bank', 'shape', 'frequency_hz', 'gain_db',
             'q', 'output_gain', 'adaptive_q'}


class DatasetError(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise DatasetError(message)


def fields(value, keys, name):
    require(isinstance(value, dict) and set(value) == set(keys), f'{name}: exact fields required')


def text(value, name, maximum=1024):
    require(isinstance(value, str) and 0 < len(value) <= maximum and value.strip()
            and '\0' not in value, f'{name}: bounded nonempty text required')


def number(value, name, minimum=0.0, maximum=1e30):
    require(type(value) in (int, float) and math.isfinite(value)
            and minimum <= value <= maximum, f'{name}: finite bounded number required')
    return float(value)


def digest(value, size, name):
    require(isinstance(value, str) and len(value) == size
            and all(c in '0123456789abcdef' for c in value), f'{name}: lowercase hex required')


def read(path):
    try:
        with path.open('rb') as source:
            data = source.read(MAX_BYTES + 1)
    except OSError as error:
        raise DatasetError(f'missing/unreadable dataset {path}: {error.strerror}') from error
    require(len(data) <= MAX_BYTES, f'dataset exceeds {MAX_BYTES} bytes')
    return data


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f'duplicate JSON field {key}')
        result[key] = value
    return result


def decode_json(data):
    try:
        return json.loads(data, object_pairs_hook=unique_object,
                          parse_constant=lambda _: (_ for _ in ()).throw(DatasetError('nonfinite JSON')))
    except (ValueError, UnicodeError, RecursionError) as error:
        raise DatasetError(f'malformed JSON: {error}') from error


def validate_case(case):
    fields(case, CASE_KEYS, 'case')
    require(type(case['sample_rate']) is int and 8000 <= case['sample_rate'] <= 192000,
            'sample_rate: current EQ supports 8000..192000')
    require((case['mode'], case['bank']) in
            (('stereo', 'stereo'), ('lr', 'left'), ('lr', 'right'), ('ms', 'mid'), ('ms', 'side')),
            'mode/bank mismatch')
    require(case['shape'] in SHAPES, 'unknown shape')
    number(case['frequency_hz'], 'frequency_hz', 20, 20000)
    number(case['gain_db'], 'gain_db', -24, 24)
    number(case['q'], 'q', 0.1, 18)
    number(case['output_gain'], 'output_gain', 0, 16)
    require(type(case['adaptive_q']) is bool, 'adaptive_q: Boolean required')
    for key in ('frequency_hz', 'gain_db', 'q', 'output_gain'):
        # Model configuration is authored f32; compare its exact f32 encoding later.
        struct.pack('<f', case[key])


def validate_bound(bound, name):
    fields(bound, ('absolute_linear', 'relative_linear'), name)
    for key in bound:
        number(bound[key], name + '.' + key, 0, 1)


def samples(data, rate):
    try:
        rows = csv.reader(io.StringIO(data.decode('utf-8')), strict=True)
        require(next(rows, None) == ['query_hz', 'lane0_linear', 'lane1_linear'], 'exact CSV header required')
        points = []
        for row in rows:
            require(len(row) == 3, 'CSV requires exactly three values per point')
            point = tuple(float(v) for v in row)
            require(all(math.isfinite(v) for v in point), 'nonfinite CSV value')
            require(0 < point[0] < rate / 2, 'query must be positive and below Nyquist')
            require(0 <= point[1] <= 1e30 and 0 <= point[2] <= 1e30, 'invalid linear magnitude')
            require(not points or point[0] > points[-1][0], 'queries must strictly increase without duplicates')
            points.append(point)
            require(len(points) <= MAX_POINTS, 'too many response points')
        require(points, 'empty response dataset')
        return points
    except (ValueError, UnicodeError, csv.Error) as error:
        raise DatasetError(f'malformed response CSV: {error}') from error


def load_manifest(path):
    raw = read(path)
    manifest = decode_json(raw)
    fields(manifest, ('schema', 'kind', 'case', 'provenance', 'uncertainty', 'tolerance'), 'manifest')
    require(type(manifest['schema']) is int and manifest['schema'] == 1, 'unsupported manifest schema')
    require(manifest['kind'] in ('reference', 'synthetic'), 'kind must be reference or synthetic')
    validate_case(manifest['case'])
    provenance = manifest['provenance']
    fields(provenance, ('producer', 'version', 'executable_sha256', 'method', 'capture'), 'provenance')
    for key in ('producer', 'version', 'method'):
        text(provenance[key], key)
    digest(provenance['executable_sha256'], 64, 'executable_sha256')
    fields(provenance['capture'], ('path', 'sha256'), 'capture')
    capture = provenance['capture']
    text(capture['path'], 'capture.path', 256)
    relative = Path(capture['path'])
    require(not relative.is_absolute() and '..' not in relative.parts, 'capture must remain inside manifest directory')
    resolved = path.parent.resolve() / relative
    require(resolved.resolve().is_relative_to(path.parent.resolve()), 'capture symlink escapes manifest directory')
    digest(capture['sha256'], 64, 'capture.sha256')
    capture_data = read(resolved)
    require(hashlib.sha256(capture_data).hexdigest() == capture['sha256'], 'capture SHA256 mismatch')
    validate_bound(manifest['uncertainty'], 'uncertainty')
    validate_bound(manifest['tolerance'], 'tolerance')
    return manifest, samples(capture_data, manifest['case']['sample_rate']), hashlib.sha256(raw).hexdigest()


def load_hex(path):
    raw = read(path)
    header, separator, data = raw.partition(b'\n')
    require(separator and header.startswith(b'# '), 'missing Hex export metadata')
    metadata = decode_json(header[2:])
    fields(metadata, ('schema', 'producer', 'state_version', 'source_sha', 'dirty', 'case'), 'Hex metadata')
    require(type(metadata['schema']) is int and metadata['schema'] == 1
            and metadata['producer'] == 'hex.eq8 prepared response', 'unsupported Hex export')
    require(type(metadata['state_version']) is int and metadata['state_version'] == 1, 'unsupported Hex state version')
    digest(metadata['source_sha'], 40, 'Hex source_sha')
    require(metadata['dirty'] is False, 'dirty Hex source is not comparison evidence')
    validate_case(metadata['case'])
    require(metadata['case']['adaptive_q'] is False, 'unsupported adaptive Hex export')
    return metadata, samples(data, metadata['case']['sample_rate']), hashlib.sha256(raw).hexdigest()


def equivalent_cases(a, b):
    for key in CASE_KEYS:
        if key in ('frequency_hz', 'gain_db', 'q', 'output_gain'):
            if struct.pack('<f', a[key]) != struct.pack('<f', b[key]):
                return False
        elif a[key] != b[key]:
            return False
    return True


def compare(manifest_path, hex_path):
    manifest, measured, manifest_sha = load_manifest(manifest_path)
    metadata, actual, hex_sha = load_hex(hex_path)
    require(not manifest['case']['adaptive_q'], 'adaptive-on dataset cannot establish comparison: Hex law/toggle is unimplemented')
    require(equivalent_cases(manifest['case'], metadata['case']), 'authored case mismatch')
    require(len(measured) == len(actual), 'point count mismatch; no interpolation permitted')
    results = []
    for measured_point, actual_point in zip(measured, actual):
        require(measured_point[0] == actual_point[0], 'query grid mismatch; no resampling permitted')
        for lane in (0, 1):
            ref, model = measured_point[lane + 1], actual_point[lane + 1]
            error = abs(model - ref)
            uncertainty = manifest['uncertainty']['absolute_linear'] + manifest['uncertainty']['relative_linear'] * ref
            tolerance = manifest['tolerance']['absolute_linear'] + manifest['tolerance']['relative_linear'] * ref
            status = ('within_tolerance' if error + uncertainty <= tolerance else
                      'outside_tolerance' if error - uncertainty > tolerance else 'inconclusive')
            results.append({'query_hz': measured_point[0], 'lane': lane, 'reference_linear': ref,
                            'hex_linear': model, 'absolute_error': error, 'uncertainty': uncertainty,
                            'tolerance': tolerance, 'status': status})
    statuses = {r['status'] for r in results}
    status = ('outside_tolerance' if 'outside_tolerance' in statuses else
              'inconclusive' if 'inconclusive' in statuses else 'within_tolerance')
    return {'schema': 1, 'kind': manifest['kind'], 'status': status, 'parity_claim': False,
            'manifest_sha256': manifest_sha, 'hex_export_sha256': hex_sha,
            'hex': metadata, 'provenance': manifest['provenance'], 'points': results}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('manifest', type=Path)
    parser.add_argument('hex_export', type=Path)
    args = parser.parse_args()
    try:
        report = compare(args.manifest, args.hex_export)
        print(json.dumps(report, sort_keys=True, indent=2, allow_nan=False))
        return {'within_tolerance': 0, 'outside_tolerance': 1, 'inconclusive': 3}[report['status']]
    except (DatasetError, OSError, OverflowError, TypeError) as error:
        print(f'comparison refused: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
