import hashlib
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile


def run(*argv):
    subprocess.run(argv, check=True, timeout=60)


def verify(image, report):
    digest = hashlib.sha256(image.read_bytes()).hexdigest()
    if digest != '9c1b943b0b93857f640d4538bf66f8a6ef4935950db68bdb0775a96b474f48a0':
        raise RuntimeError(f'Unexpected disk image SHA-256: {digest}')
    source_dir = Path(__file__).parent
    units = runpy.run_path(str(source_dir / 'verify-macos-package.py'))['units']
    with tempfile.TemporaryDirectory(prefix='sdir-rename-') as temporary:
        root = Path(temporary)
        helper_source = root / 'main.swift'
        common = (source_dir / 'verify-macos-ui.swift').read_text()
        main = '\nlet args = CommandLine.arguments\n'
        if common.count(main) != 1:
            raise RuntimeError('Swift helper entry point changed')
        helper_source.write_text(common.split(main)[0] + '\n' + (source_dir / 'verify-macos-rename.swift').read_text())
        helper = root / 'verify-rename'
        run('/usr/bin/xcrun', 'swiftc', '-swift-version', '5', str(helper_source), '-o', str(helper))

        mount = root / 'disk'
        mount.mkdir()
        run('/usr/bin/hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount), str(image))
        try:
            installed = root / 'Applications' / 'SDIR.app'
            run('/usr/bin/ditto', str(mount / 'SDIR.app'), str(installed))
            run('/usr/bin/codesign', '--verify', '--strict', str(installed))
        finally:
            run('/usr/bin/hdiutil', 'detach', str(mount))
        folder = root / 'SDIR Receiving Folder'
        finder_source = root / 'Finder Source'
        profile = root / 'profile'
        for directory in (folder, finder_source, profile):
            directory.mkdir()
        (folder / 'readiness-fixture.txt').write_bytes(b'SDIR fixture readiness')
        dragged = finder_source / 'SDIR Finder drop.txt'
        dragged.write_bytes(b'0.6.3 macOS Finder drop fixture\n')
        settings = {
            'version': 1, 'layout': 1, 'panes': [[units(folder)]],
            'sidebar_visible': False, 'action_bar_visible': False,
            'window': {'x': 0, 'y': 30, 'width': 980, 'height': 620, 'maximized': False},
        }
        (profile / 'settings.json').write_text(json.dumps(settings))

        executable = installed / 'Contents/MacOS/SDIR'
        with report.with_name('sdir-gui.log').open('w') as log, subprocess.Popen(
            [str(executable)], env={**os.environ, 'SDIR_PROFILE_DIR': str(profile)},
            stdout=log, stderr=subprocess.STDOUT,
        ) as app:
            try:
                events_path = report.with_name('native-events.jsonl')
                with events_path.open('w') as event_log:
                    try:
                        result = subprocess.run(
                            [str(helper), str(app.pid), str(folder), str(dragged),
                             str(report.with_name('window-failure.png'))],
                            stdout=event_log, stderr=log, timeout=420)
                        helper_exit = result.returncode
                    except subprocess.TimeoutExpired:
                        helper_exit = 124
                events = [json.loads(line) for line in events_path.read_text().splitlines()]
                for event in events:
                    print(json.dumps(event, ensure_ascii=False))
                try:
                    app_exit = app.wait(timeout=6)
                except subprocess.TimeoutExpired:
                    app_exit = None
                report.write_text(json.dumps({
                    'sha256': digest, 'os': os.uname().sysname, 'arch': os.uname().machine,
                    'helperExit': helper_exit, 'appExit': app_exit, 'journeys': events,
                }, ensure_ascii=False, indent=2) + '\n')
                expected = {f'rename_{index:03d}' for index in range(70)} | {'finder_drop'}
                passed = {event['journey'] for event in events if event['status'] == 'passed'}
                return helper_exit == 0 and passed == expected
            finally:
                if app.poll() is None:
                    app.terminate()
                    try:
                        app.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        app.kill()
                        app.wait()


if __name__ == '__main__':
    image, report = (Path(argument).resolve() for argument in sys.argv[1:])
    report.parent.mkdir(parents=True, exist_ok=True)
    raise SystemExit(0 if verify(image, report) else 1)
