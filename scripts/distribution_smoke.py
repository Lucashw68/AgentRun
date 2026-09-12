#!/usr/bin/env python3
"""Test an archive in Linux userlands; containers share the host kernel."""
import argparse
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--archive', type=Path, required=True)
parser.add_argument('--image', action='append', help='Override the default distribution images')
args = parser.parse_args()
images = args.image or ['ubuntu:24.04', 'fedora:44', 'debian:13-slim', 'alpine:3.23']
if not args.image and platform.machine() == 'x86_64':
    images.append('archlinux:latest')
script = Path(__file__).with_suffix('.sh').read_text()
with tempfile.TemporaryDirectory(prefix='agentrun-distributions-') as temp:
    root = Path(temp)
    with tarfile.open(args.archive) as archive:
        archive.extractall(root, filter='data')
    directories = list(root.iterdir())
    if len(directories) != 1 or not directories[0].is_dir():
        raise SystemExit('Expected one archive root directory')
    bundle = directories[0]
    for image in images:
        print(f'Testing {image}', flush=True)
        subprocess.run(['docker', 'run', '--rm', '-i', '--network', 'none',
                        '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
                        '--pids-limit', '128', '--user', '1000:1000',
                        '--tmpfs', '/tmp:rw,exec,nosuid,nodev,size=64m',
                        '--mount', f'type=bind,src={bundle},dst=/bundle,readonly',
                        image, 'sh', '-s'], input=script, text=True, check=True, timeout=300)
