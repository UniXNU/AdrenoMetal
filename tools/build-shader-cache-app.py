#!/usr/bin/env python3
"""Build the macOS Shader Cache app and its administrator-authorized worker."""
import argparse
import json
import plistlib
from pathlib import Path
import re
import shutil
import subprocess

SOURCE = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('output', type=Path)
parser.add_argument('--arch', choices=('arm64', 'arm64e'), default='arm64e')
args = parser.parse_args()
app = args.output.resolve() / 'Shader Cache.app'
if app.exists():
    shutil.rmtree(app)
code = app / 'Contents/MacOS'
code.mkdir(parents=True)
sdk = subprocess.check_output(['xcrun', '--sdk', 'macosx', '--show-sdk-path'], text=True).strip()
base = ['xcrun', 'clang++', '-target', args.arch + '-apple-macos27.0', '-isysroot', sdk,
        '-std=c++17', '-fobjc-arc', '-O2', '-Wall', '-Wextra', '-Werror', '-Wno-deprecated-declarations', '-Wno-unused-function']
hashes = {}
for name, frameworks in [('ShaderCache', ('AppKit', 'Security')), ('ShaderCacheWorker', ('Foundation', 'Metal')),
                         ('ShaderCacheRegister', ('Foundation', 'CoreServices'))]:
    source = 'ShaderCacheApp.mm' if name == 'ShaderCache' else name + '.mm'
    subprocess.run([*base, SOURCE / 'ShaderCache' / source,
                    *[part for framework in frameworks for part in ('-framework', framework)],
                    *(['-framework', 'IOKit'] if name == 'ShaderCacheWorker' else []), '-o', code / name], check=True)
    entitlements = args.output / (name + '-entitlements.plist')
    grants = {'com.apple.security.iokit-user-client-class': ['AGXDeviceUserClient'],
              'com.apple.private.security.no-container': True} if name == 'ShaderCacheWorker' and args.arch == 'arm64e' else {}
    entitlements.write_bytes(plistlib.dumps(grants))
    subprocess.run(['codesign', '--force', '--sign', '-', '--timestamp=none', '--pagesize', '4096',
                    '--entitlements', entitlements, code / name], check=True)
    detail = subprocess.run(['codesign', '-d', '--verbose=4', code / name], capture_output=True, text=True, check=True).stderr
    hashes[name] = re.search(r'^CDHash=([0-9a-f]{40})$', detail, re.M)[1]
    entitlements.unlink()
(app / 'Contents/Info.plist').write_bytes(plistlib.dumps(dict(
    CFBundleIdentifier='org.unixnu.ShaderCache', CFBundleName='Shader Cache', CFBundleDisplayName='Shader Cache',
    CFBundleExecutable='ShaderCache', CFBundlePackageType='APPL', CFBundleVersion='1', CFBundleShortVersionString='1.0',
    LSMinimumSystemVersion='27.0', NSHighResolutionCapable=True, NSPrincipalClass='NSApplication')))
# The outer seal retains the executable's existing entitlements.
subprocess.run(['codesign', '--force', '--sign', '-', '--timestamp=none', '--pagesize', '4096',
                '--preserve-metadata=entitlements', app], check=True)
for path in (app, code / 'ShaderCacheWorker', code / 'ShaderCacheRegister'):
    subprocess.run(['codesign', '--verify', '--strict', path], check=True)
detail = subprocess.run(['codesign', '-d', '--verbose=4', app], capture_output=True, text=True, check=True).stderr
hashes['ShaderCache'] = re.search(r'^CDHash=([0-9a-f]{40})$', detail, re.M)[1]
(args.output / 'shader-cache-trust.json').write_text(json.dumps(hashes, indent=2) + '\n')
print(app)
