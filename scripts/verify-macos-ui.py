import hashlib
import json
import os
import runpy
import subprocess
import sys
import tempfile
from pathlib import Path


EXPECTED_SHA256 = '4567c43186f219187f4a65976ea42faee699898fbb8c347b0ab799bbd34bd76c'


def run(*argv):
    subprocess.run(argv, check=True, timeout=60)


def verify(image, helper, report):
    digest = hashlib.sha256(image.read_bytes()).hexdigest()
    if digest != EXPECTED_SHA256:
        raise RuntimeError(f'Unexpected disk image SHA-256: {digest}')
    units = runpy.run_path(str(Path(__file__).with_name('verify-macos-package.py')))['units']
    with tempfile.TemporaryDirectory(prefix='sdir-gui-') as temporary:
        root = Path(temporary)
        mount = root / 'disk'
        mount.mkdir()
        run('/usr/bin/hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount), str(image))
        try:
            installed = root / 'Applications' / 'SDIR.app'
            run('/usr/bin/ditto', str(mount / 'SDIR.app'), str(installed))
            run('/usr/bin/codesign', '--verify', '--strict', str(installed))
        finally:
            run('/usr/bin/hdiutil', 'detach', str(mount))
        source = root / 'Finder Source'
        target = root / 'SDIR Receiving Folder'
        profile = root / 'profile'
        for directory in (source, target, profile):
            directory.mkdir()
        dragged = source / 'SDIR Finder drop.txt'
        dragged.write_bytes(b'SDIR 0.6.1 native Finder drop fixture\n')
        (target / 'start.txt').write_bytes(b'receiving panel fixture\n')
        settings = {
            'version': 1,
            'layout': 1,
            'panes': [[units(target)]],
            'pane_names': ['검증 패널'],
            'sidebar_visible': False,
            'action_bar_visible': False,
            'window': {'x': 0, 'y': 30, 'width': 980, 'height': 620, 'maximized': False},
        }
        (profile / 'settings.json').write_text(json.dumps(settings), encoding='utf-8')
        app = installed / 'Contents' / 'MacOS' / 'SDIR'
        gui_log = report.with_name('sdir-gui.log')
        with gui_log.open('w', encoding='utf-8') as log:
            with subprocess.Popen([str(app)], env={**os.environ, 'SDIR_PROFILE_DIR': str(profile)}, stdout=log, stderr=subprocess.STDOUT) as child:
                try:
                    result = subprocess.run([str(helper), str(child.pid), str(dragged), str(target)], capture_output=True, text=True, timeout=95)
                    events = []
                    for line in result.stdout.splitlines():
                        print(line)
                        events.append(json.loads(line))
                    if result.stderr.strip():
                        print(result.stderr.strip(), file=sys.stderr)
                    try:
                        app_exit = child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        app_exit = None
                    expected = {'window_launch', 'korean_ime', 'finder_drop'}
                    seen = {event['journey'] for event in events}
                    receipt = {'sha256': digest, 'helperExit': result.returncode, 'appExit': app_exit, 'journeys': events}
                    report.write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
                    return result.returncode == 0 and seen == expected and all(event['status'] == 'passed' for event in events)
                finally:
                    if child.poll() is None:
                        child.terminate()
                        try:
                            child.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            child.kill()
                            child.wait()


if __name__ == '__main__':
    image, helper, report = (Path(argument).resolve() for argument in sys.argv[1:])
    report.parent.mkdir(parents=True, exist_ok=True)
    raise SystemExit(0 if verify(image, helper, report) else 1)
