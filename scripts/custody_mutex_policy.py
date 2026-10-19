"""Audited exactly-five-field custody primitive, not a general lock facility."""
from pathlib import Path
import re
import tomllib

BASE = 'crates/hex-audio/src/candidate_custody'
ROOT = BASE + '.rs'
ELIGIBLE = BASE + '/eligible.rs'
WRAPPER = BASE + '/custody_mutex.rs'
CALLBACK_FILES = {
    BASE + '/eligible/publication/active.rs',
    BASE + '/eligible/publication/streaming/actions.rs',
    BASE + '/eligible/publication/streaming/demand.rs',
}
EXPECTED = {
    ROOT: {'bank': 'Bank', 'execution': 'Work', 'returns': 'Option<AbandonedReturns>'},
    ELIGIBLE: {'bank': 'NativeBank', 'work': 'NativeWork'},
}


def dependency_violations(package, manifest):
    result = []
    sections = [(kind, manifest.get(kind, {}))
                for kind in ('dependencies', 'dev-dependencies', 'build-dependencies')]
    for target in manifest.get('target', {}).values():
        sections += [('target-' + kind, target.get(kind, {}))
                     for kind in ('dependencies', 'dev-dependencies', 'build-dependencies')]
    found = 0
    for kind, dependencies in sections:
        for alias, spec in dependencies.items():
            name = spec.get('package', alias) if isinstance(spec, dict) else alias
            if name != 'spin':
                continue
            found += 1
            valid = (package == 'hex-audio' and kind == 'dependencies'
                     and alias == 'spin' and isinstance(spec, dict)
                     and spec.get('version') == '=0.9.9'
                     and spec.get('default-features') is False
                     and set(spec.get('features', [])) == {'mutex', 'spin_mutex', 'std'}
                     and len(spec.get('features', [])) == 3
                     and set(spec) == {'version', 'default-features', 'features'})
            if not valid:
                result.append(f'{package}: spin requires the exact five-field Audio dependency contract')
    if package == 'hex-audio' and found != 1:
        result.append('hex-audio: exactly one normal pinned spin dependency is required')
    return result


def source_violations(path, source):
    result = []
    if path in EXPECTED:
        fields = dict(re.findall(r'^\s*(\w+): CustodyMutex<(.+)>,\s*$', source, re.M))
        if fields != EXPECTED[path]:
            result.append(f'{path}: exactly the declared custody fields must use CustodyMutex')
    uses = re.search(r'\b(?:spin\s*::|CustodyMutex\b|CustodyMutexGuard\b|lock_recover_on_control\b)', source)
    diagnostic_test = (path == BASE + '/eligible/publication/tests.rs'
                       and not re.search(r'\bCustodyMutex\b|\bspin\s*::|lock_recover_on_control', source))
    allowed = (path in {ROOT, ELIGIBLE, WRAPPER, BASE + '/inline_lock_tests.rs'}
               or path.startswith(BASE + '/custody_mutex/') or diagnostic_test)
    if uses and not allowed:
        result.append(f'{path}: custody primitive is private to its five fields and owned tests')
    if path == WRAPPER:
        if ('SpinMutex<T, spin::Yield>' not in source
                or 'PhantomData<Rc<()>>' not in source
                or len(re.findall(r'self\s*\.value\s*\.try_lock\(\)', source)) != 1):
            result.append('custody primitive must keep explicit Yield/nonSend/one-try source contract')
        start = source.find('fn try_lock(')
        end = source.find('\n    }', start)
        attempt = source[start:end]
        if re.search(r'\b(?:while|for|loop|yield_now)\b|\.lock\s*\(', attempt):
            result.append('custody try_lock must not retry, yield or block')
        if re.search(r'\bunsafe\s+(?:impl|fn|trait|\{)|\*(?:const|mut)\b|force_unlock|into_inner|data_ptr', source):
            result.append('custody primitive must not extract owners or add unsafe access')
    if path in CALLBACK_FILES and re.search(r'\block\s*\(|\.lock\s*\(|lock_recover_on_control|yield_now', source):
        result.append(f'{path}: callback custody paths must not yield/wait or call control lock recovery')
    if path == ROOT and source.count('value.lock_recover_on_control()') != 1:
        result.append('only the existing custody control helper routes yielding recovery')
    if path in {ROOT, ELIGIBLE} and 'prewarm_on_control' in source:
        result.append(f'{path}: custody locks require no first-lock warmup')
    return result


def repository_violations(root: Path):
    result = []
    for path in sorted((root / 'crates').glob('*/Cargo.toml')):
        manifest = tomllib.loads(path.read_text())
        result += dependency_violations(manifest['package']['name'], manifest)
    for path in sorted((root / 'crates').rglob('*.rs')):
        result += source_violations(path.relative_to(root).as_posix(), path.read_text())
    return result
