#!/usr/bin/env python3
"""Offline failure tests for the downloader; real archive, tar and installer.

Only the HTTPS transfer is substituted. Real GitHub downloads are checked
separately before release; no HTTP mirror or insecure mode is added to production.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--archive', type=Path, required=True)
args = parser.parse_args()
source = args.archive.resolve()
installer = Path(__file__).with_name('download-install.sh').resolve()
version = source.name.split('-')[1]
archive_root = source.name.removesuffix('.tar.gz')
original = source.read_bytes()
with tempfile.TemporaryDirectory(prefix='agentrun-download-test-') as temp:
    root = Path(temp)
    commands = root / 'commands'
    commands.mkdir()
    payload = root / source.name
    payload.write_bytes(original)
    requests = root / 'requests.jsonl'
    home = root / 'home'
    home.mkdir()
    downloads = root / 'downloads'
    downloads.mkdir()
    curl = commands / 'curl'
    curl.write_text('''#!/usr/bin/env python3
import hashlib,json,os,sys
from pathlib import Path
args=sys.argv[1:]
assert args[0]=='-q'
assert args[args.index('--proto')+1]=='=https'
assert args[args.index('--proto-redir')+1]=='=https'
assert '--insecure' not in args and '-k' not in args
url=args[-1]
assert url.startswith('https://github.com/example/AgentRun/releases/')
with open(os.environ['REQUEST_LOG'],'a') as f: f.write(json.dumps(url)+'\\n')
mode=os.environ.get('TRANSFER_MODE','ok')
if mode=='offline': sys.exit(22)
if '--write-out' in args:
 print('https://github.com/example/AgentRun/releases/tag/v'+os.environ['RELEASE_VERSION'],end='')
 sys.exit(0)
source=Path(os.environ['PAYLOAD'])
data=source.read_bytes()
if url.endswith('.sha256'):
 digest=hashlib.sha256(data).hexdigest() if mode!='checksum' else '0'*64
 data=(digest+'  '+source.name+'\\n').encode()
else:
 assert url.endswith('/'+source.name)
Path(args[args.index('--output')+1]).write_bytes(data)
''')
    curl.chmod(0o755)
    gh = commands / 'gh'
    gh.write_text('''#!/usr/bin/env python3
import hashlib,json,os,sys
from pathlib import Path
args=sys.argv[1:]
assert args[0]=='release'
assert args[args.index('--repo')+1]=='https://github.com/example/AgentRun'
with open(os.environ['REQUEST_LOG'],'a') as f: f.write(json.dumps('gh:'+args[1])+'\\n')
if os.environ.get('TRANSFER_MODE')=='offline': sys.exit(1)
if args[1]=='view':
 print('v'+os.environ['RELEASE_VERSION']); sys.exit(0)
assert args[1]=='download'
source=Path(os.environ['PAYLOAD'])
name=args[args.index('--pattern')+1]
data=source.read_bytes()
if name.endswith('.sha256'):
 digest=hashlib.sha256(data).hexdigest() if os.environ.get('TRANSFER_MODE')!='checksum' else '0'*64
 data=(digest+'  '+source.name+'\\n').encode()
else: assert name==source.name
(Path(args[args.index('--dir')+1])/name).write_bytes(data)
''')
    gh.chmod(0o755)
    uname = commands / 'uname'
    uname.write_text('#!/bin/sh\ncase "$1" in -s) echo Linux;; -r) echo "${TEST_KERNEL:-6.12.0}";; -m) echo "${TEST_ARCH}";; esac\n')
    uname.chmod(0o755)
    env = dict(os.environ, HOME=str(home), TMPDIR=str(downloads),
               PATH=str(commands) + os.pathsep + os.environ['PATH'],
               REQUEST_LOG=str(requests), PAYLOAD=str(payload),
               RELEASE_VERSION=version, TEST_ARCH=platform.machine())
    prefix = root / 'installed with spaces'
    config = home / '.config/agentrun/config.json'
    config.parent.mkdir(parents=True)
    config.write_text('preserve policy')

    def run(*extra, overrides=None):
        result = subprocess.run(['sh', str(installer), '--repo', 'example/AgentRun',
                                 '--prefix', str(prefix), *extra],
                                env=dict(env, **(overrides or {})), capture_output=True,
                                text=True, timeout=20)
        assert not list(downloads.iterdir()), result.stderr
        assert config.read_text() == 'preserve policy'
        return result

    result = run()
    assert result.returncode == 0, result.stderr
    assert json.loads(requests.read_text().splitlines()[0]).endswith('/latest')
    assert subprocess.check_output([prefix / 'bin/agentrun', '--version'], text=True).strip() == f'agentrun {version}'
    requests.write_text('')
    assert run('--version', 'v'+version).returncode == 0
    assert '/latest' not in requests.read_text()
    snapshots = {name: (prefix / 'bin' / name).read_bytes()
                 for name in ['agentrun', 'agentrun-mcp', 'agentrun-log']}
    requests.write_text('')
    assert run('--github-cli').returncode == 0
    assert requests.read_text().splitlines() == ['"gh:view"', '"gh:download"', '"gh:download"']
    assert run('--github-cli', '--version', version).returncode == 0

    def refused(*extra, overrides=None):
        result = run(*extra, overrides=overrides)
        assert result.returncode != 0, result.stdout
        for name, data in snapshots.items():
            assert (prefix / 'bin' / name).read_bytes() == data

    refused(overrides={'TRANSFER_MODE': 'checksum'})
    refused(overrides={'TRANSFER_MODE': 'offline'})
    refused('--github-cli', overrides={'TRANSFER_MODE': 'offline'})
    refused('--github-cli', overrides={'TRANSFER_MODE': 'checksum'})
    refused('--repo', 'example/../another')
    refused('--version', '../0.3.2')
    refused(overrides={'TEST_KERNEL': '6.8.0'})
    refused(overrides={'TEST_ARCH': 'riscv64'})
    refused(overrides={'RELEASE_VERSION': '../../malicious'})

    def rewritten(mode):
        with tarfile.open(fileobj=io.BytesIO(original)) as package, tarfile.open(payload, 'w:gz') as out:
            for member in package:
                if member.name == archive_root + '/agentrun-log':
                    if mode == 'missing':
                        continue
                    if mode == 'symlink':
                        member.type = tarfile.SYMTYPE
                        member.linkname = '/usr/bin/true'
                        member.size = 0
                        out.addfile(member)
                        continue
                    if mode == 'duplicate':
                        out.addfile(member, package.extractfile(member))
                out.addfile(member, package.extractfile(member))
            if mode == 'traversal':
                member = tarfile.TarInfo(str(root / 'escaped'))
                member.size = 3
                out.addfile(member, io.BytesIO(b'bad'))

    for mode in ['missing', 'symlink', 'duplicate']:
        rewritten(mode)
        refused('--version', version)
    rewritten('traversal')
    assert run('--version', version).returncode == 0
    assert not (root / 'escaped').exists()
print('Downloader: public/private, latest, pinned version, HTTPS options, cleanup and failure preservation checks passed')
