"""Root-only Mac absent-target probe. Never creates/opens an app or targets live Hex."""
from dataclasses import asdict
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time

EXPECTED_HEAD = 'a1a4391e9f5e502a22716423c5bd122a5c9f2290'
EXPECTED_TREE = '54d12aa9f05d4aa2a176bf29a5577657f2576862'
EXPECTED_SWIFT = '5e1cf00760ca6d73f91f0131ce86cf61012a2db46dccc35c99180c69f41d79d0'

def git(root, *args):
    return subprocess.check_output(['git', *args], cwd=root, text=True).strip()

def main():
    if sys.platform != 'darwin' or len(sys.argv) != 3:
        raise RuntimeError('root Mac only: pass frozen source checkout and an exclusively owned report parent')
    root = Path(sys.argv[1]).resolve()
    if git(root, 'rev-parse', 'HEAD') != EXPECTED_HEAD or git(root, 'rev-parse', 'HEAD^{tree}') != EXPECTED_TREE:
        raise RuntimeError('wrong source/tree; no native operation')
    if git(root, 'status', '--porcelain=v1', '--untracked-files=all'):
        raise RuntimeError('source dirty; no native operation')
    if hashlib.sha256((root/'scripts/launch-restart.swift').read_bytes()).hexdigest() != EXPECTED_SWIFT:
        raise RuntimeError('wrong Swift source; no native operation')
    # Resolve ONLY this newly created, exclusively owned fixture. Do not relax
    # or canonicalize any arbitrary application/control target in production.
    fixture = Path(tempfile.mkdtemp(prefix='hex-413-compiled-absent-', dir=sys.argv[2])).resolve()
    output = fixture/'report'; output.mkdir(mode=0o700)
    app = fixture/'hex.app'
    if app.exists():
        raise RuntimeError('fixture app must be absent')
    spec = importlib.util.spec_from_file_location('hex_413_native_probe', root/'scripts/launch-restart.py')
    module = importlib.util.module_from_spec(spec);sys.modules[spec.name] = module;spec.loader.exec_module(module)
    class AbsentOnly:
        def verify(self, path):
            raise RuntimeError('unexpected installed bundle; probe refused')
    controller = module.NativeController(root, app, output, AbsentOnly())
    record = {'source':EXPECTED_HEAD,'tree':EXPECTED_TREE,'swift_sha256':EXPECTED_SWIFT,
              'kind':'compiled helper absent-target only; no app behavior/listening proof',
              'status':'incomplete','operations':[]}
    started = time.monotonic()
    try:
        snapshot = controller.snapshot()  # ACTUAL production lazy compilation.
        if snapshot != []:
            raise RuntimeError('nonempty snapshot; no further operations')
        record['operations'].append({'operation':'snapshot','exit':0,'result':snapshot})
        provenance = json.loads((output/'native-helper.json').read_text())
        record['provenance'] = provenance
        if (provenance['source_sha256'] != EXPECTED_SWIFT
                or provenance['compile_timeout_seconds'] != 120
                or provenance['operation_timeout_seconds'] != 15):
            raise RuntimeError('unexpected compilation source/budgets')
        binary = controller._binary
        identity = controller._binary_identity
        record['binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
        if record['binary_sha256'] != provenance['binary_sha256']:
            raise RuntimeError('binary changed before probes')
        expected = module.Identity(2147483647,'absent-fixture',str(app),str(app/'Contents/MacOS/hex'),'org.portalsurfer.hex.dev')
        for operation in ['quit','force']:
            if app.exists():
                raise RuntimeError('fixture app appeared; refusal')
            # NativeController.call itself rightly refuses actions when there
            # is no installed seal. This DIRECT helper-protocol probe is ONLY
            # for an absent private path/invented identity. It is not evidence
            # of the production normal-quit/force path on a running app.
            argv = [str(binary),operation,str(app),json.dumps(asdict(expected),separators=(',',':'))]
            result = controller._run_owned(argv,15,operation)
            controller._validate_helper()
            record['operations'].append({'operation':operation,'argv':argv,'exit':result.returncode,
                                         'stdout':result.stdout,'stderr':result.stderr})
            if result.returncode != 1 or 'application identity changed; no termination requested' not in result.stderr:
                raise RuntimeError('absent identity was not refused as expected')
            if controller._binary_identity != identity:
                raise RuntimeError('compiled helper identity changed')
        if (output/'native-compile.log').read_text().count('compile: exit 0;') != 1:
            raise RuntimeError('expected exactly one successful compilation')
        record['status'] = 'pass'
    except BaseException as error:
        record.update(status='failed',error=str(error));raise
    finally:
        private = controller._helper_dir
        try:
            controller.close()
        except BaseException as error:
            record.update(status='failed',cleanup_error=str(error))
            raise
        finally:
            record['private_workspace_removed'] = private is None or not private.exists()
            record['elapsed_seconds'] = time.monotonic()-started
            (output/'native-probe.json').write_text(json.dumps(record,indent=2)+'\n')
            print(output, flush=True)
    if not record['private_workspace_removed']:
        raise RuntimeError('private helper workspace not removed')

if __name__ == '__main__':
    main()
