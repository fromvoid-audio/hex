#!/usr/bin/env python3
"""Validate the frozen native profiler CSV and emit source-external JSON evidence."""
import argparse
import csv
import hashlib
import io
import json
import math
import statistics
from pathlib import Path

OPERATIONS = ('copy', 'cut', 'replace-paste', 'ripple-insert')
TIMINGS = (
    'selection_first_paint_us', 'selection_following_frame_us',
    'operation_first_paint_us', 'operation_first_following_frame_us',
    'operation_ui_completion_paint_us', 'operation_completion_following_frame_us',
)
FIELDS = ['tracks', 'operation', 'iteration', 'warmup', 'selection_start_frame',
          'selection_end_frame', *TIMINGS, 'project_revision', 'failures']

def analyze(raw):
    reader = csv.DictReader(io.StringIO(raw.decode('utf-8')))
    if reader.fieldnames != FIELDS:
        raise ValueError('unexpected CSV schema')
    rows = list(reader)
    if len(rows) != 360:
        raise ValueError(f'expected 360 rows, received {len(rows)}')
    seen = set()
    selected_ranges = set()
    groups = {(tracks, operation): [] for tracks in (1, 100) for operation in OPERATIONS}
    for row in rows:
        tracks, iteration = int(row['tracks']), int(row['iteration'])
        key = (tracks, row['operation'], iteration)
        if tracks not in (1, 100) or row['operation'] not in OPERATIONS or not 0 <= iteration < 45 or key in seen:
            raise ValueError(f'invalid or duplicate case {key}')
        seen.add(key)
        expected_warmup = 'true' if iteration < 5 else 'false'
        if row['warmup'] != expected_warmup or int(row['failures']) != 0:
            raise ValueError(f'warmup/failure mismatch {key}')
        if not 0 <= int(row['selection_start_frame']) < int(row['selection_end_frame']):
            raise ValueError(f'invalid selection {key}')
        selected_ranges.add((int(row['selection_start_frame']), int(row['selection_end_frame'])))
        if int(row['project_revision']) < 0:
            raise ValueError(f'invalid project revision {key}')
        values = {field: int(row[field]) for field in TIMINGS}
        if min(values.values()) <= 0:
            raise ValueError(f'nonpositive timing {key}')
        for paint, following in zip(TIMINGS[::2], TIMINGS[1::2]):
            if values[following] < values[paint]:
                raise ValueError(f'following-frame bound precedes paint {key}: {paint}')
        if values['operation_first_paint_us'] > values['operation_ui_completion_paint_us']:
            raise ValueError(f'feedback follows completion {key}')
        if values['operation_first_following_frame_us'] > values['operation_completion_following_frame_us']:
            raise ValueError(f'feedback following-frame bound follows completion bound {key}')
        if expected_warmup == 'false':
            groups[(tracks, row['operation'])].append(values)
    if len(selected_ranges) != 1:
        raise ValueError(f'selected frame range differs across rows: {sorted(selected_ranges)}')
    summary = []
    for (tracks, operation), group in groups.items():
        if len(group) != 40:
            raise ValueError(f'expected 40 measured samples for {(tracks, operation)}')
        metrics = {}
        for field in TIMINGS:
            values = sorted(item[field] for item in group)
            metrics[field] = {
                'median': statistics.median(values),
                'p95': values[math.ceil(.95 * len(values)) - 1],
                'p99': values[math.ceil(.99 * len(values)) - 1],
                'minimum': values[0], 'maximum': values[-1],
                'mean': statistics.mean(values),
            }
        summary.append({'tracks': tracks, 'operation': operation, 'measured_samples': 40,
                        'warmup_samples': 5, 'metrics_us': metrics})
    lookup = {(item['tracks'], item['operation']): item for item in summary}
    ratios = {}
    for operation in OPERATIONS:
        ratios[operation] = {}
        for field in TIMINGS:
            small = lookup[(1, operation)]['metrics_us'][field]['median']
            dense = lookup[(100, operation)]['metrics_us'][field]['median']
            ratios[operation][field] = dense / small if small else None
    return {
        'raw_sha256': hashlib.sha256(raw).hexdigest(), 'raw_rows': 360,
        'measured_rows': 320, 'warmup_rows': 40, 'failures': 0,
        'median_method': 'arithmetic median of middle two ordered samples',
        'p95_method': 'nearest rank ceil(0.95*n), 38th of 40',
        'p99_method': 'nearest rank ceil(0.99*n), 40th of 40, equal to observed maximum',
        'tail_caveat': 'n=40 per case; p99 equals observed maximum and does not establish rare-event tail reliability',
        'selected_frame_range': list(next(iter(selected_ranges))),
        'timing_boundary': 'native canvas paint; following-frame observation after scene submission',
        'completion_boundary': 'UI-observed completion, including normal 16ms worker polling',
        'excludes': ['physical pixels', 'compositor completion', 'GPU completion', 'audio deadlines'],
        'cases': summary, 'dense_to_small_median_ratio': ratios,
    }

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('raw_csv', type=Path)
    parser.add_argument('summary_json', type=Path)
    args = parser.parse_args()
    result = analyze(args.raw_csv.read_bytes())
    with args.summary_json.open('x') as output:
        json.dump(result, output, indent=2, allow_nan=False)
        output.write('\n')
    print(f'validated {result["raw_rows"]} rows; {result["measured_rows"]} measured; failures=0')

if __name__ == '__main__':
    main()
