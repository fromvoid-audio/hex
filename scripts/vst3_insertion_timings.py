#!/usr/bin/env python3
"""Report control-side VST3 insertion timings for serial attempts in one Hex session.

Pass the session log and any rotated sibling logs in any order. CSV times are
milliseconds since UI submission. Paint and editor replies are diagnostic
events; physical window visibility must be timed separately.
"""
import argparse
import csv
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

UI = re.compile(r'vst3_load v=1 stage=(\w+) class=("(?:\\.|[^"\\])*") '
                r'generation=(\d+) revision=(\d+) rate_hz=(\d+) destination=(\S+)')
NATIVE = re.compile(r'vst3_insert v=1 stage=(\w+) generation=(\d+) '
                    r'binding=ForeignProcessorBinding \{ root: (\d+), instance: (\d+), '
                    r'graph: (\d+), epoch: (\d+) \}(.*)')
STAGES = {
    'loading_feedback_painted': 'paint', 'native_prepare_returned': 'prepared',
    'native_facts': 'facts', 'endpoint_ready': 'endpoint', 'graph_terminal': 'installed',
    'automatic_editor_queued': 'editor_queued', 'automatic_editor_terminal': 'editor_reply',
}
PREP = ('selection_inspection', 'revalidation', 'waitable_probe', 'instance_to_facts')
FIELDS = ['session', 'source', 'sequence', 'class', 'generation', 'revision', 'rate_hz',
          'destination', 'root', 'instance', 'graph', 'epoch', 'status', 'reason',
          *(s + '_ms' for s in STAGES.values()), *(s + '_ms' for s in PREP)]


def ms(nanos):
    return f'{int(nanos) / 1_000_000:.6f}'


@dataclass
class Attempt:
    identity: tuple
    start: int
    sequence: int
    binding: tuple = ()
    times: dict = field(default_factory=dict)
    breakdown: dict = field(default_factory=dict)
    errors: set = field(default_factory=set)
    terminal: str = ''
    refusal: str = ''

    def mark(self, stage, elapsed):
        if stage in self.times or elapsed < self.start:
            self.errors.add('duplicate or invalid time: ' + stage)
        else:
            self.times[stage] = elapsed

    def finished(self):
        return self.terminal == 'refused' or (
            self.terminal == 'accepted' and (self.refusal or 'editor_reply' in self.times))

    def record_native(self, native, elapsed):
        stage, gen, root, instance, graph, epoch, details = native.groups()
        if (int(gen), int(graph) - 1) != self.identity[1:3] or stage not in STAGES:
            return
        binding = (root, instance, graph, epoch)
        if self.binding and self.binding != binding:
            self.errors.add('ambiguous native binding')
            return
        self.binding = binding
        if stage in ('graph_terminal', 'automatic_editor_terminal'):
            key = 'committed' if stage == 'graph_terminal' else 'open'
            if details == f' {key}=false':
                if self.refusal or STAGES[stage] in self.times:
                    self.errors.add('conflicting or duplicate terminal')
                self.refusal = 'graph refused' if stage == 'graph_terminal' else 'editor refused'
                return
            if details != f' {key}=true' or self.refusal:
                self.errors.add('invalid terminal')
                return
        self.mark(STAGES[stage], elapsed)
        if stage == 'native_prepare_returned':
            for key in PREP:
                duration = re.search(rf' {key}_ns=(?:Some\((\d+)\)|(\d+)|None)(?: |$)', details)
                if not duration:
                    self.errors.add('missing preparation detail: ' + key)
                elif duration[1] or duration[2]:
                    self.breakdown[key + '_ms'] = ms(duration[1] or duration[2])

    def row(self, session, source):
        name, generation, revision, rate, dest = self.identity
        missing = [s for s in STAGES.values() if s not in self.times]
        # Paint can race preparation. Native custody stages themselves are ordered.
        native_times = [self.times[s] for s in list(STAGES.values())[1:] if s in self.times]
        if native_times != sorted(native_times):
            self.errors.add('native stages out of order')
        if self.errors:
            status, reason = 'incomplete', ', '.join(sorted(self.errors))
        elif self.refusal or self.terminal == 'refused':
            status, reason = 'refused', self.refusal or 'UI refused'
        elif missing or self.terminal != 'accepted':
            status, reason = 'incomplete', 'missing: ' + ', '.join(missing + ([] if self.terminal else ['UI terminal']))
        else:
            status, reason = 'complete', ''
        return {
            'session': session, 'source': source, 'sequence': self.sequence, 'class': name,
            'generation': generation, 'revision': revision, 'rate_hz': rate, 'destination': dest,
            **dict(zip(('root', 'instance', 'graph', 'epoch'), self.binding)),
            'status': status, 'reason': reason,
            **{key + '_ms': ms(value - self.start) for key, value in self.times.items()},
            **self.breakdown,
        }


def segment_header(stream):
    prefix, sep, context = stream.readline(32_768).rstrip('\n').partition(' context=')
    if not prefix.startswith('session_started ') or not sep:
        raise ValueError('missing session header')
    fields = dict(part.split('=', 1) for part in prefix.split()[1:])
    if fields['log_schema'] != '3' or fields['context_truncated'] != 'false':
        raise ValueError('requires a complete schema-3 session header')
    source = re.search(r'(?:^| )source=(\S+)', json.loads(context))
    if not source:
        raise ValueError('missing build source')
    return fields['session'], source[1], int(fields['segment'])


def attempts(paths):
    headers = {}
    for path in paths:
        # hex-diagnostics rotates segments at 8 MiB. Bound reads to ordinary
        # files so a malformed input cannot masquerade as an unbounded stream.
        if not path.is_file() or path.stat().st_size > 8 * 1024 * 1024:
            raise ValueError(f'expected a regular Hex log segment up to 8 MiB: {path}')
        with path.open(encoding='utf-8') as stream:
            header = segment_header(stream)
        if header in headers:
            raise ValueError('duplicate segment')
        headers[header] = path
    if len({header[:2] for header in headers}) != 1:
        raise ValueError('expected segments from one session and source')

    current = None
    previous_sequence = previous_segment = None
    for (session, source, segment), path in sorted(headers.items()):
        with path.open(encoding='utf-8') as stream:
            if segment_header(stream) != (session, source, segment):
                raise ValueError('log rotated during analysis')
            if previous_segment is not None and segment != previous_segment + 1 and current and not current.finished():
                current.errors.add('missing segment')
            previous_segment = segment
            for line in stream:
                if line.startswith('diagnostics_loss ') and current and not current.finished():
                    current.errors.add('logger reported loss')
                if not line.startswith('event '):
                    continue
                prefix, sep, encoded = line.rstrip('\n').partition(' message=')
                fields = dict(part.split('=', 1) for part in prefix.split()[1:])
                sequence, elapsed = int(fields['sequence']), int(fields['elapsed_nanos'])
                if previous_sequence is not None and sequence != previous_sequence + 1 and current and not current.finished():
                    current.errors.add('event sequence discontinuity')
                previous_sequence = sequence
                if not sep:
                    raise ValueError(f'event {sequence} has no message')
                if not encoded.startswith(('"vst3_load ', '"vst3_insert ')):
                    continue
                if fields['truncated'] != 'false':
                    raise ValueError(f'truncated insertion event {sequence}')
                message = json.loads(encoded)
                ui = UI.fullmatch(message)
                if ui:
                    if fields['category'] != 'ui':
                        continue
                    stage, name, gen, rev, rate, dest = ui.groups()
                    try:
                        label = json.loads(name)
                    except ValueError:
                        label = name  # Preserve Rust Debug escapes outside JSON's repertoire.
                    identity = (label, int(gen), int(rev), int(rate), dest)
                    if stage == 'submitted':
                        overlap = current is not None and not current.finished()
                        ambiguous = overlap and current.identity[1:3] == identity[1:3]
                        if current:
                            if overlap:
                                current.errors.add('next submission before completion')
                            yield current.row(session, source)
                        current = Attempt(identity, elapsed, sequence)
                        if ambiguous:
                            current.errors.add('ambiguous repeated submission')
                    elif current and current.identity == identity:
                        if stage == 'loading_feedback_painted':
                            current.mark('paint', elapsed)
                        elif stage in ('accepted', 'refused'):
                            if current.terminal:
                                current.errors.add('duplicate UI terminal')
                            current.terminal = stage
                    continue

                native = NATIVE.fullmatch(message)
                if not native:
                    raise ValueError(f'unrecognized insertion record at sequence {sequence}')
                if current and fields['category'] == 'audio':
                    current.record_native(native, elapsed)
    if current:
        yield current.row(session, source)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('logs', type=Path, nargs='+')
    args = parser.parse_args()
    try:
        rows = list(attempts(args.logs))
        if not rows:
            raise ValueError('no VST3 insertion submissions in the supplied segments')
    except (ValueError, KeyError, OSError) as error:
        parser.exit(1, f'cannot analyze insertion logs: {error}\n')
    writer = csv.DictWriter(sys.stdout, fieldnames=FIELDS)
    writer.writeheader()
    writer.writerows(rows)


if __name__ == '__main__':
    main()
