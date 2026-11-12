#!/usr/bin/env python3
"""Alternate two optimized dense navigation probes at matching viewport events."""

import argparse
from contextlib import ExitStack
import os
from pathlib import Path
from queue import Empty, Queue
import subprocess
from threading import Thread
import time

TEST = 'editor::clip_content_cache::profile::navigation::headless_navigation_framework_profile'
FRAMES = tuple((action, frame) for action in ('pan', 'zoom') for frame in range(48))
FIRST_LABELS = ('A', 'B', 'B', 'A')
TIMEOUT_SECONDS = 30.0


def remaining(deadline):
    seconds = deadline - time.monotonic()
    if seconds <= 0:
        raise TimeoutError('navigation interleave exceeded 30 seconds')
    return seconds


class Probe:
    def __init__(self, label, executable, log, env, cwd):
        self.label = label
        self.gates = Queue()
        self.passed = False
        self.process = subprocess.Popen(
            [str(executable), TEST, '--ignored', '--exact', '--nocapture', '--test-threads=1'],
            cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        self.reader = Thread(target=self.read_output, args=(log,), daemon=True)
        self.reader.start()

    def read_output(self, log):
        try:
            for line in self.process.stdout:
                log.write(line)
                if line.startswith(b'NAV_READY,'):
                    self.gates.put(line)
                if b'test result: ok. 1 passed; 0 failed' in line:
                    self.passed = True
        except OSError as error:
            self.gates.put(error)
        finally:
            self.gates.put(None)

    def wait(self, event, deadline):
        try:
            actual = self.gates.get(timeout=remaining(deadline))
        except Empty as error:
            raise TimeoutError(f'{self.label} timed out at {event}') from error
        if isinstance(actual, OSError):
            raise actual
        if event is None and actual is None:
            if self.process.wait(timeout=remaining(deadline)) != 0 or not self.passed:
                raise RuntimeError(f'{self.label} failed the navigation probe')
            return
        expected = None if event is None else f'NAV_READY,{event[0]},{event[1]}\n'.encode()
        if actual != expected or actual is None:
            raise RuntimeError(f'{self.label} expected {expected!r}, received {actual!r}')

    def advance(self, next_event, deadline):
        remaining(deadline)
        self.process.stdin.write(b'GO\n')
        self.process.stdin.flush()
        self.wait(next_event, deadline)

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        self.reader.join()
        self.process.stdin.close()
        self.process.stdout.close()


def run_pair(executables, replica, first, output, env, cwd, deadline):
    second = 'B' if first == 'A' else 'A'
    with ExitStack() as stack:
        probes = []
        for label in (first, second):
            remaining(deadline)
            log = stack.enter_context((output / f'{replica:02d}-{label}.log').open('xb'))
            probe = Probe(label, executables[label], log, env, cwd)
            stack.callback(probe.close)
            # Wait for the first process to park before starting its peer.
            probe.wait(FRAMES[0], deadline)
            probes.append(probe)

        for i in range(len(FRAMES)):
            following = FRAMES[i + 1] if i + 1 < len(FRAMES) else None
            # Only release the peer after the active process has parked again.
            probes[i % 2].advance(following, deadline)
            probes[1 - i % 2].advance(following, deadline)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('a_executable', type=Path, help='optimized hex-ui test executable A')
    parser.add_argument('b_executable', type=Path, help='optimized hex-ui test executable B')
    parser.add_argument('audio_fixture_dir', type=Path, help='generated fixture directory containing 100.hex')
    parser.add_argument('output_dir', type=Path, help='existing directory for eight fresh raw trial logs')
    args = parser.parse_args()

    start = time.monotonic()
    deadline = start + TIMEOUT_SECONDS
    executables = {'A': args.a_executable.resolve(strict=True),
                   'B': args.b_executable.resolve(strict=True)}
    for label, executable in executables.items():
        if not executable.is_file() or not os.access(executable, os.X_OK):
            parser.error(f'{label} must be an executable file')
    fixture = args.audio_fixture_dir.resolve(strict=True)
    if not (fixture / '100.hex').is_file():
        parser.error('audio fixture directory must contain 100.hex')
    cwd = Path.cwd() / 'crates' / 'hex-ui'
    if not cwd.is_dir():
        parser.error('run this controller from the repository root')
    output = args.output_dir.resolve(strict=True)
    env = {key: value for key, value in os.environ.items() if not key.startswith('HEX_NAVIGATION_')}
    env.update(HEX_NAVIGATION_SCOPE='audio-100-dense', HEX_NAVIGATION_AUDIO_FIXTURE_DIR=str(fixture),
               HEX_NAVIGATION_INTERLEAVE='1', HEX_NAVIGATION_STAGE_TIMING='0', HEX_NAVIGATION_COMPARE_LAYOUTS='0')

    for replica, first in enumerate(FIRST_LABELS):
        run_pair(executables, replica, first, output, env, cwd, deadline)
    remaining(deadline)
    print(f'INTERLEAVE_COMPLETE,replicas={len(FIRST_LABELS)},seconds={time.monotonic() - start:.3f}')


if __name__ == '__main__':
    main()
