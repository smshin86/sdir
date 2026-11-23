#!/usr/bin/env bash
set -euo pipefail

if [[ $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    printf '%s\n' 'Build on a native Apple Silicon macOS host.' >&2
    exit 1
fi

cd "$(dirname "$0")/.."
project_root=$PWD
export MACOSX_DEPLOYMENT_TARGET=13.0
stage=$(mktemp -d "${TMPDIR:-/tmp}/sdir-build.XXXXXXXX")
trap 'rm -r -- "$stage"' EXIT

cargo build --locked --release --features desktop --bin sdir --target aarch64-apple-darwin
cargo metadata --locked --no-deps --format-version 1 > "$stage/metadata.json"
version=$(python3 - "$stage/metadata.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding='utf-8') as source:
    print(next(package['version'] for package in json.load(source)['packages'] if package['name'] == 'sdir'))
PY
)
cargo_output=$(python3 - "$stage/metadata.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding='utf-8') as source:
    print(json.load(source)['target_directory'])
PY
)

bundle="$stage/image/SDIR.app/Contents"
mkdir -p "$bundle/MacOS" "$bundle/Resources" "$stage/SDIR.iconset"
install -m 755 "$cargo_output/aarch64-apple-darwin/release/sdir" "$bundle/MacOS/SDIR"
python3 - "$project_root/assets/sdir.ico" "$bundle/Info.plist" "$stage/icon.png" "$version" <<'PY'
import plistlib, struct, sys
from pathlib import Path
ico, plist, icon, version = sys.argv[1:]
data = Path(ico).read_bytes()
images = []
for index in range(struct.unpack_from('<H', data, 4)[0]):
    width, height, _, _, _, depth, size, start = struct.unpack_from('<BBBBHHII', data, 6 + index * 16)
    payload = data[start:start + size]
    if payload.startswith(b'\x89PNG\r\n\x1a\n'):
        images.append(((width or 256) * (height or 256), depth, payload))
if not images:
    raise SystemExit('SDIR ICO contains no PNG image')
Path(icon).write_bytes(max(images)[2])
with open(plist, 'wb') as out:
    plistlib.dump({
        'CFBundleDevelopmentRegion': 'ko',
        'CFBundleDisplayName': 'SDIR',
        'CFBundleExecutable': 'SDIR',
        'CFBundleIconFile': 'SDIR.icns',
        'CFBundleIdentifier': 'com.smshin86.sdir',
        'CFBundleInfoDictionaryVersion': '6.0',
        'CFBundleName': 'SDIR',
        'CFBundlePackageType': 'APPL',
        'CFBundleShortVersionString': version,
        'CFBundleVersion': version,
        'LSMinimumSystemVersion': '13.0',
        'NSHighResolutionCapable': True,
    }, out)
PY

for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$stage/icon.png" --out "$stage/SDIR.iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$stage/icon.png" --out "$stage/SDIR.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil --convert icns "$stage/SDIR.iconset" --output "$bundle/Resources/SDIR.icns"
cp README.md THIRD_PARTY_NOTICES.md THIRD_PARTY_LICENSES.txt "$bundle/Resources/"
cargo metadata --locked --features desktop --filter-platform aarch64-apple-darwin --format-version 1 > "$stage/macos-dependencies.json"
python3 - "$stage/macos-dependencies.json" "$bundle/Resources/THIRD_PARTY_LICENSES.txt" <<'PYTHON'
import json, sys
from pathlib import Path
metadata, notices = map(Path, sys.argv[1:])
existing = notices.read_text(encoding='utf-8').splitlines()
with notices.open('a', encoding='utf-8') as out:
    packages = json.loads(metadata.read_text(encoding='utf-8'))['packages']
    for package in sorted(packages, key=lambda item: (item['name'], item['version'])):
        title = f"{package['name']} {package['version']}"
        if package['source'] is None or title in existing or package['name'].startswith('goz-'):
            continue
        print('\n==============================================================================', title, package['license'] or '', package['repository'] or '', sep='\n', file=out)
        root = Path(package['manifest_path']).parent
        files = sorted(path for path in root.iterdir() if path.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE')) and path.is_file())
        if package['license_file']:
            path = root / package['license_file']
            if path.is_file() and path not in files:
                files.append(path)
        for path in files:
            print(f'\n--- {path.name} ---', path.read_text(encoding='utf-8', errors='replace'), sep='\n', file=out)
PYTHON
ln -s /Applications "$stage/image/Applications"

codesign --force --sign - "$stage/image/SDIR.app"
codesign --verify --strict "$stage/image/SDIR.app"
plutil -lint "$bundle/Info.plist"
[[ $(lipo -archs "$bundle/MacOS/SDIR") == arm64 ]]

name="SDIR-macos-arm64-$version.dmg"
mkdir -p dist
hdiutil create -volname SDIR -srcfolder "$stage/image" -format UDZO "$stage/$name"
hdiutil verify "$stage/$name"
install -m 644 "$stage/$name" "dist/$name"
printf '%s\n' "dist/$name"
