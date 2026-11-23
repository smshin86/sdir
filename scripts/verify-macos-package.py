import json
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path


def run(*argv, **kwargs):
    return subprocess.run(argv, check=True, timeout=90, **kwargs)


def verify_disk_image(image):
    with tempfile.TemporaryDirectory(prefix='sdir-package-check-') as temp:
        root = Path(temp)
        mount = root / 'volume'
        mount.mkdir()
        run('/usr/bin/hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount), str(image))
        try:
            if os.readlink(mount / 'Applications') != '/Applications':
                raise RuntimeError('The disk image does not point to the Applications folder')
            target = root / 'Applications' / 'SDIR.app'
            shutil.copytree(mount / 'SDIR.app', target, symlinks=True)
            run('/usr/bin/codesign', '--verify', '--strict', str(target))
            contents = target / 'Contents'
            with (contents / 'Info.plist').open('rb') as handle:
                info = plistlib.load(handle)
            executable = contents / 'MacOS' / info['CFBundleExecutable']
            if info['CFBundlePackageType'] != 'APPL' or not (contents / 'Resources' / 'SDIR.icns').is_file():
                raise RuntimeError('The installed application bundle is incomplete')
            expected_image = f"SDIR-macos-arm64-{info['CFBundleShortVersionString']}.dmg"
            if image.name != expected_image:
                raise RuntimeError(f'Bundle version and disk image filename differ: {expected_image}')
            if run('/usr/bin/lipo', '-archs', str(executable), capture_output=True, text=True).stdout.strip() != 'arm64':
                raise RuntimeError('The installed executable does not match Apple Silicon')
            verify_listing(executable, root)
            print(f'Installed bundle signature, version, icon, architecture and file listing passed: {image.name}')
        finally:
            run('/usr/bin/hdiutil', 'detach', str(mount))


def units(value):
    encoded = os.fspath(value).encode('utf-16-le')
    return [int.from_bytes(encoded[index:index + 2], 'little') for index in range(0, len(encoded), 2)]


def verify_listing(executable, root):
    fixture = root / 'listing fixture'
    fixture.mkdir()
    filename = '회의 메모.txt'
    (fixture / filename).write_bytes(b'SDIR package verification\n')
    request = {'type': 'list', 'pane': 0, 'id': 7, 'path': units(fixture), 'hidden': False}
    try:
        response = run(str(executable), '--worker', input=json.dumps(request) + '\n', capture_output=True, text=True)
    except subprocess.CalledProcessError as error:
        raise RuntimeError(f'The installed listing worker failed: {error.stderr.strip()}') from error
    events = [json.loads(line) for line in response.stdout.splitlines()]
    complete = [{'type': 'listed', 'pane': 0, 'id': 7, 'error': None}]
    if [event for event in events if event['type'] == 'listed'] != complete:
        raise RuntimeError('The installed application did not finish listing the fixture')
    names = [
        unicodedata.normalize('NFC', entry[0])
        for event in events if event['type'] == 'batch'
        for entry in event['entries'] if isinstance(entry[0], str)
    ]
    if names != [filename]:
        raise RuntimeError('The installed application did not list the expected Unicode file')


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit('Usage: python3 scripts/verify-macos-package.py SDIR-macos-arm64-VERSION.dmg')
    verify_disk_image(Path(sys.argv[1]).resolve(strict=True))
