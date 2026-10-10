#!/usr/bin/env python3
"""Check declared crate boundaries through Cargo metadata; no native build needed."""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
ALLOWED = {
    'hex-lattice-contract': set(),
    'hex-project': {'hex-lattice-contract'},
    'hex-lattice': {'hex-lattice-contract'},
    'hex-devices': {'hex-lattice', 'hex-lattice-contract'},
    'hex-media': {'hex-engine', 'hex-project'},
    'hex-storage': {'hex-project', 'hex-lattice-contract'},
    'hex-session': {'hex-project', 'hex-media', 'hex-engine', 'hex-storage', 'hex-export', 'hex-lattice-contract'},
    'hex-engine': {'hex-project', 'hex-devices', 'hex-lattice', 'hex-lattice-contract'},
    'hex-stream': {'hex-engine', 'hex-media', 'hex-project'},
    'hex-export': {'hex-engine'},
    'hex-library': set(),
    'hex-preferences': set(),
    'hex-widgets': set(),
    'hex-audio': {'hex-engine', 'hex-stream'},
    'hex-ui': {'hex-engine', 'hex-audio', 'hex-session', 'hex-project', 'hex-library', 'hex-preferences', 'hex-lattice-contract', 'hex-widgets'},
    'hex-desktop': {'hex-ui', 'hex-audio'},
    'hex-cli': {'hex-engine', 'hex-project', 'hex-media', 'hex-storage', 'hex-export'},
}
# Canonical publication fixtures need the authored model only in audio tests.
# This does not grant the production device/callback crate ownership of editing.
DEV_ALLOWED = {'hex-audio': {'hex-project', 'hex-lattice-contract', 'hex-lattice'}}
BOUNDARIES = {'gpui': {'hex-ui', 'hex-desktop', 'hex-widgets'},
              'gpui_platform': {'hex-ui', 'hex-desktop', 'hex-widgets'},
              'cpal': {'hex-audio'}, 'rtrb': {'hex-audio', 'hex-stream'},
              # Raw source-page fingerprints belong to control-side media, never DSP.
              'sha2': {'hex-media'}}
metadata = json.loads(subprocess.check_output(
    ['cargo', 'metadata', '--format-version', '1', '--no-deps'], cwd=ROOT))
packages = {p['name']: p for p in metadata['packages'] if p['id'] in metadata['workspace_members']}
errors = []
for name, package in packages.items():
    if name not in ALLOWED:
        errors.append(f'{name}: register a purpose and dependency policy before adding this crate')
        continue
    readme = Path(package['manifest_path']).parent / 'README.md'
    content = readme.read_text() if readme.exists() else ''
    for heading in ('## Purpose', '## Dependencies', '## Does not own'):
        if heading not in content:
            errors.append(f'{name}: README must define {heading}')
    for dep in package['dependencies']:
        target = dep['name']
        if target == 'allocation-counter' and (name not in {'hex-audio', 'hex-stream'} or dep['kind'] != 'dev'):
            errors.append(f'{name}: allocation-counter is allowed only for audio/stream dev probes')
        allowed = ALLOWED[name]
        if dep['kind'] == 'dev':
            allowed = allowed | DEV_ALLOWED.get(name, set())
        if target in packages and target not in allowed:
            errors.append(f'{name} -> {target}: violates the dependency policy')
        if target in BOUNDARIES and name not in BOUNDARIES[target]:
            errors.append(f'{name}: {target} belongs in its declared platform/UI boundary')
if errors:
    print('\n'.join(errors), file=sys.stderr)
    sys.exit(1)
print(f'architecture policy passed: {len(packages)} crates with explicit purposes and boundaries')
