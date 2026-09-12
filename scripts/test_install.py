#!/usr/bin/env python3
"""Exercise the offline installer with real binaries and temporary prefixes."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--archive', type=Path, required=True)
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='agentrun-install-test-') as temp:
    root = Path(temp)
    with tarfile.open(args.archive) as archive:
        archive.extractall(root, filter='data')
    bundle = next(root.iterdir())
    home = root / 'home'
    home.mkdir()
    env = dict(os.environ, HOME=str(home))
    config = home / '.config/agentrun/config.json'
    config.parent.mkdir(parents=True)
    config.write_text('preserve user policy\n')

    def install(*arguments, environment=env):
        return subprocess.run(['sh', str(bundle / 'install.sh'), *arguments],
                              env=environment, capture_output=True, text=True, timeout=15)

    assert install().returncode == 0
    assert config.read_text() == 'preserve user policy\n'
    for name in ('agentrun', 'agentrun-mcp', 'agentrun-log', 'agentrun-setup'):
        assert (home / '.local/bin' / name).read_bytes() == (bundle / name).read_bytes()
    assert install('--prefix', 'relative').returncode != 0
    prefix = root / 'prefix with spaces'
    assert install('--prefix', str(prefix)).returncode == 0
    assert install('--prefix', str(prefix)).returncode == 0  # upgrade/reinstall

    # A failed preflight must not partially replace the other binaries.
    (prefix / 'bin/agentrun').write_text('existing CLI')
    (prefix / 'bin/agentrun-log').unlink()
    sentinel = root / 'sentinel'
    sentinel.write_text('do not overwrite')
    (prefix / 'bin/agentrun-log').symlink_to(sentinel)
    assert install('--prefix', str(prefix)).returncode != 0
    assert sentinel.read_text() == 'do not overwrite'
    assert (prefix / 'bin/agentrun').read_text() == 'existing CLI'

    # The utility must pass the same no-clobber preflight as the binaries.
    utility_prefix = root / 'utility-symlink'
    assert install('--prefix', str(utility_prefix)).returncode == 0
    (utility_prefix / 'bin/agentrun').write_text('existing CLI')
    (utility_prefix / 'bin/agentrun-setup').unlink()
    (utility_prefix / 'bin/agentrun-setup').symlink_to(sentinel)
    assert install('--prefix', str(utility_prefix)).returncode != 0
    assert (utility_prefix / 'bin/agentrun').read_text() == 'existing CLI'
    assert sentinel.read_text() == 'do not overwrite'

    # Only the installer platform probe is substituted, never Core process tests.
    commands = root / 'commands'
    commands.mkdir()
    uname = commands / 'uname'
    uname.write_text('#!/bin/sh\ncase "$1" in -s) echo Linux;; -r) echo 6.8.0-test;; esac\n')
    uname.chmod(0o755)
    rejected = root / 'old-kernel'
    old_env = dict(env, PATH=str(commands) + os.pathsep + env['PATH'])
    assert install('--prefix', str(rejected), environment=old_env).returncode != 0
    assert not rejected.exists()

    missing = root / 'incomplete'
    shutil.copytree(bundle, missing)
    (missing / 'agentrun-log').unlink()
    result = subprocess.run(['sh', str(missing / 'install.sh'), '--prefix', str(rejected)],
                            env=env, capture_output=True, timeout=15)
    assert result.returncode != 0 and not rejected.exists()
print('Installer: normal install, upgrade, spaces, existing policy, symlink, kernel and incomplete archive checks passed')
