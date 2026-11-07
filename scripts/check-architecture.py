#!/usr/bin/env python3
"""Check declared crate boundaries through Cargo metadata; no native build needed."""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
ALLOWED = {
    'hex-mts': {'hex-tuning'},
    'hex-tuning': set(),
    'hex-dsp': set(),
    'hex-midi': set(),
    # Standalone scanner executables share one Linux child supervisor; no library
    # or host may depend on it because it owns process-wide child disposition.
    'hex-scan-supervisor': set(),
    'hex-clap-scan': {'hex-scan-supervisor'},
    'hex-plugin-scan': {'hex-scan-supervisor'},
    'hex-plugin-catalog': {'hex-plugin-scan'},
    'hex-plugin-state': {'hex-lattice-contract'},
    # Ordinary control/worker client; never callback, process-global signals or graph authority.
    'hex-plugin-host': {'hex-plugin-scan', 'hex-plugin-state', 'hex-lattice-contract'},
    'hex-diagnostics': set(),
    'hex-lattice-contract': set(),
    'hex-project': {'hex-lattice-contract', 'hex-plugin-state', 'hex-tuning'},
    'hex-lattice': {'hex-lattice-contract', 'hex-dsp', 'hex-tuning'},
    'hex-devices': {'hex-lattice', 'hex-lattice-contract', 'hex-dsp'},
    'hex-media': {'hex-engine', 'hex-project'},
    'hex-storage': {'hex-project', 'hex-lattice-contract', 'hex-tuning'},
    # Pure captured-save envelope validation; no native runtime/ABI ownership.
    'hex-session': {'hex-tuning', 'hex-project', 'hex-media', 'hex-engine', 'hex-storage', 'hex-export', 'hex-lattice-contract', 'hex-plugin-state'},
    'hex-warp': set(),
    'hex-engine': {'hex-warp', 'hex-project', 'hex-devices', 'hex-lattice', 'hex-lattice-contract'},
    'hex-stream': {'hex-engine', 'hex-media', 'hex-project', 'hex-storage'},
    'hex-export': {'hex-engine'},
    'hex-library': set(),
    'hex-preferences': set(),
    'hex-widgets': set(),
    # Disposable native preview mounts the production UI owners, without audio/session.
    'hex-sandbox': {'hex-devices', 'hex-preferences', 'hex-project', 'hex-widgets'},
    # Concrete catalog/pair custody and control disposal, never an engine->stream edge.
    'hex-audio': {'hex-engine', 'hex-stream', 'hex-plugin-host', 'hex-plugin-state', 'hex-plugin-scan', 'hex-lattice', 'hex-lattice-contract'},
    'hex-ui': {'hex-plugin-catalog', 'hex-mts', 'hex-tuning', 'hex-midi', 'hex-diagnostics', 'hex-engine', 'hex-audio', 'hex-session', 'hex-project', 'hex-library', 'hex-preferences', 'hex-lattice-contract', 'hex-widgets'},
    # Desktop injects the Session sink and binds neutral source facts to actual
    # audio/stream constructor custody; Session has no device/stream edge.
    'hex-desktop': {'hex-mts', 'hex-midi', 'hex-diagnostics', 'hex-ui', 'hex-audio', 'hex-engine', 'hex-session', 'hex-stream', 'hex-media', 'hex-export', 'hex-plugin-catalog', 'hex-preferences'},
    'hex-cli': {'hex-engine', 'hex-project', 'hex-media', 'hex-storage', 'hex-export'},
}
# These narrow test-only edges exercise canonical publication and prepared
# mutable-instrument fixtures. They do not grant production audio/export code
# ownership of editing or device implementations.
DEV_ALLOWED = {
    # Real generic project/file/history round trips, never runtime state I/O.
    'hex-storage': {'hex-plugin-state'},
    # Real decoded WAV -> sealed source scope -> sampler PCM oracles only.
    'hex-media': {'hex-devices', 'hex-lattice', 'hex-lattice-contract'},
    # Verified worker/key-handler fixtures and the legacy native EQ gallery example.
    'hex-ui': {'hex-media', 'hex-devices'},
    'hex-engine': {'hex-tuning'},
    'hex-session': {'hex-tuning'},
    # Actual Session -> native adapter fixtures, never a production editor model.
    'hex-desktop': {'hex-project', 'hex-tuning', 'hex-storage', 'hex-plugin-state'},
    # Actual provider/pair callback allocation probes; never a production edge.
    'hex-audio': {'hex-project', 'hex-storage', 'hex-lattice-contract', 'hex-lattice', 'hex-media', 'hex-tuning', 'hex-devices', 'hex-plugin-scan', 'hex-plugin-state'},
    'hex-export': {'hex-project', 'hex-lattice-contract', 'hex-lattice', 'hex-tuning'},
}
BOUNDARIES = {'mts-esp-master-adapter': {'hex-mts'}, 'libloading': set(), 'coremidi': {'hex-midi'}, 'gpui': {'hex-ui', 'hex-desktop', 'hex-widgets', 'hex-sandbox'},
              'gpui_platform': {'hex-ui', 'hex-desktop', 'hex-widgets', 'hex-sandbox'},
              'cpal': {'hex-audio'}, 'rtrb': {'hex-audio', 'hex-stream', 'hex-plugin-host'},
              # Safe macOS joining-transport scheduling, never callback calls.
              'qos-threads': {'hex-audio'},
              # Source/scanner identities, automation draft integrity and bounded
              # control-side hashes of retained PCM and selected desktop update receipts; never callback DSP.
              'sha2': {'hex-storage', 'hex-media', 'hex-engine', 'hex-plugin-scan', 'hex-clap-scan', 'hex-desktop'},
              # Child disposition belongs only to the standalone scanners.
              'signal-hook': {'hex-plugin-scan', 'hex-clap-scan', 'hex-scan-supervisor'},
              # Signal only the owned local-update helper group on desktop control workers.
              'nix': {'hex-desktop'},
              # Safe publication/standalone child ownership; host event-only socket readiness.
              'rustix': {'hex-export', 'hex-storage', 'hex-plugin-scan', 'hex-clap-scan', 'hex-scan-supervisor', 'hex-plugin-host'},
              # Advisory document/catalog and desktop update-lifetime locks stay on control boundaries.
              'fs2': {'hex-storage', 'hex-plugin-catalog', 'hex-desktop'}}
# Build-only hash checks of observed pinned rtrb constructor files. This does
# not permit cryptography or a new dependency on the stream runtime path.
BUILD_BOUNDARIES = {'coremidi': {'hex-midi'}, 'sha2': {'hex-stream'}}
metadata = json.loads(subprocess.check_output(
    ['cargo', 'metadata', '--format-version', '1', '--no-deps'], cwd=ROOT))
packages = {p['name']: p for p in metadata['packages'] if p['id'] in metadata['workspace_members']}
errors = []
for name, package in packages.items():
    if name not in ALLOWED:
        errors.append(f'{name}: register a purpose and dependency policy before adding this crate')
        continue
    # Ownership belongs beside the implementation, not in a parallel README.
    entry = Path(package['manifest_path']).parent / 'src' / 'lib.rs'
    if not entry.exists():
        entry = entry.with_name('main.rs')
    content = entry.read_text() if entry.exists() else ''
    documentation = '\n'.join(line[3:].lstrip() for line in content.splitlines()
                              if line.startswith('//!'))
    for heading in ('# Purpose', '# Dependencies', '# Does not own'):
        sections = documentation.split(heading, 1)
        body = sections[1].split('\n# ', 1)[0].strip() if len(sections) == 2 else ''
        if not body:
            errors.append(f'{name}: crate doc comments must define {heading}')
    for dep in package['dependencies']:
        target = dep['name']
        if target == 'rustix' and 'process' in dep.get('features', []) and name not in {'hex-plugin-scan', 'hex-clap-scan', 'hex-scan-supervisor'}:
            errors.append(f'{name}: rustix process APIs belong only to the scanner supervisor')
        if target == 'allocation-counter' and (name not in {'hex-audio', 'hex-stream', 'hex-desktop', 'hex-plugin-host'} or dep['kind'] != 'dev'):
            errors.append(f'{name}: allocation-counter is allowed only for audio/stream/desktop/copied-host dev probes')
        allowed = ALLOWED[name]
        if dep['kind'] == 'dev':
            allowed = allowed | DEV_ALLOWED.get(name, set())
        if target in packages and target not in allowed:
            errors.append(f'{name} -> {target}: violates the dependency policy')
        if (target in BOUNDARIES and name not in BOUNDARIES[target]
                and not (dep['kind'] == 'build'
                         and name in BUILD_BOUNDARIES.get(target, set()))
                # Only safe CPU-clock probes, never a production audio edge.
                and not (dep['kind'] == 'dev' and name == 'hex-audio'
                         and target == 'rustix'
                         and set(dep.get('features', [])) <= {'std', 'time'})):
            errors.append(f'{name}: {target} belongs in its declared platform/UI boundary')
if errors:
    print('\n'.join(errors), file=sys.stderr)
    sys.exit(1)
print(f'architecture policy passed: {len(packages)} crates with explicit purposes and boundaries')
