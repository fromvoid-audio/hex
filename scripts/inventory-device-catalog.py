#!/usr/bin/env python3
"""Read-only installed Live metadata inventory; never execute or copy device code."""
import argparse
import hashlib
import json
import os
import plistlib
import re
from pathlib import Path

MAX_ENTRIES = 100_000
MAX_METADATA_BYTES = 256 * 1024


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()


def bounded_children(parent, remaining):
    # scandir is lazy; Path.iterdir/listdir can materialize before our cap.
    children = []
    with os.scandir(parent) as entries:
        for entry in entries:
            if len(children) >= remaining:
                raise ValueError('inventory entry limit exceeded: ' + str(parent))
            children.append(Path(entry.path))
    return sorted(children)


def bounded_metadata(path):
    if path.is_symlink():
        raise ValueError('unsafe metadata symlink: ' + str(path))
    with path.open('rb') as stream:
        content = stream.read(MAX_METADATA_BYTES + 1)
    if len(content) > MAX_METADATA_BYTES:
        raise ValueError('oversized metadata: ' + str(path))
    return content


def metadata(path):
    if not path.exists():
        return {'availability': 'missing'}
    content = bounded_metadata(path)
    text = content.decode('utf-8')
    values = {}
    for key, quoted, integer in re.findall(
        r'(?:String|Int)\s+(Pack\w+|DeviceId|DeviceClassName)\s*=\s*(?:"([^"]*)"|(-?\d+))\s*;', text
    ):
        values[key] = quoted if quoted else int(integer) if integer else ''
    return {'availability': 'present', 'sha256': hashlib.sha256(content).hexdigest(), 'values': values}


def files(root):
    if not root.is_dir() or root.is_symlink():
        raise ValueError('inventory root must be an existing real directory: ' + str(root))
    pending = [root]
    count = 0
    while pending:
        parent = pending.pop()
        for child in bounded_children(parent, MAX_ENTRIES - count):
            count += 1
            if count > MAX_ENTRIES:
                raise ValueError('inventory entry limit exceeded: ' + str(root))
            if child.is_symlink():
                raise ValueError('inventory refuses symlink: ' + str(child))
            if child.is_dir():
                pending.append(child)
            elif child.is_file():
                yield child


def inventory(app, packs):
    plist_path = app / 'Contents/Info.plist'
    plist = plistlib.loads(bounded_metadata(plist_path))
    executable = plist['CFBundleExecutable']
    if Path(executable).name != executable:
        raise ValueError('invalid executable name')
    resources = app / 'Contents/App-Resources'
    result = {
        'schema': 1,
        'method': 'read-only metadata and file hashes; no browser launch or device execution',
        'reference': {
            'bundle_version': plist['CFBundleVersion'],
            'short_version': plist['CFBundleShortVersionString'],
            'bundle_id': plist['CFBundleIdentifier'],
            'executable_sha256': digest(app / 'Contents/MacOS' / executable),
        },
        'roots': {}, 'packs': [], 'max_devices': [], 'native_preset_groups': [],
        'limitations': [
            'filesystem presence does not establish browser visibility, licensing or successful loading',
            'device control laws, defaults, latency and native acceptance require separate specifications and experiments',
            'pack vendor metadata is recorded verbatim; first-party/distribution rights are not inferred',
            'user library and third-party plugins are outside this installed factory inventory',
        ],
    }
    roots = [('builtin', resources / 'Builtin/Devices'),
             ('core', resources / 'Core Library/Devices'), ('packs', packs)]
    for label, root in roots:
        if not root.exists():
            result['roots'][label] = {'availability': 'missing'}
            continue
        all_files = list(files(root))
        result['roots'][label] = {'availability': 'present', 'file_count': len(all_files)}
        if label == 'packs':
            for pack in bounded_children(root, MAX_ENTRIES):
                if pack.is_dir():
                    result['packs'].append({'directory': pack.name,
                        'metadata': metadata(pack / 'Ableton Folder Info/Properties.cfg')})
        groups = {}
        for path in all_files:
            relative = path.relative_to(root)
            if path.suffix.lower() == '.amxd':
                result['max_devices'].append({
                    'id': label + ':' + relative.as_posix(),
                    'name': path.stem, 'root': label, 'path': relative.as_posix(),
                    'pack': relative.parts[0] if label == 'packs' else None,
                    'sha256': digest(path), 'size_bytes': path.stat().st_size,
                    'owner_ticket': 2690,
                })
            elif label == 'core' and path.suffix.lower() in ('.adv', '.adg'):
                # Preserve legacy subgroup identity; preset titles are not additional devices.
                parts = relative.parts
                group = parts[:3] if len(parts) > 3 and parts[1] == 'Legacy' else parts[:2]
                key = '/'.join(group)
                groups[key] = groups.get(key, 0) + 1
        result['native_preset_groups'].extend(
            {'root': label, 'path': key, 'preset_count': value}
            for key, value in sorted(groups.items()))
    result['max_devices'].sort(key=lambda entry: entry['id'])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=Path, default=Path('/Applications/Ableton Live 12 Suite.app'))
    parser.add_argument('--packs', type=Path, default=Path.home() / 'Music/Ableton/Factory Packs')
    args = parser.parse_args()
    try:
        print(json.dumps(inventory(args.app, args.packs), indent=2, ensure_ascii=False))
    except (OSError, ValueError, KeyError, plistlib.InvalidFileException) as error:
        parser.exit(1, 'inventory failed: ' + str(error) + '\n')


if __name__ == '__main__':
    main()
