#!/usr/bin/env python3
"""Optional Linux resource check; requires built sibling AgentRun binaries.

Use --valgrind /path/to/valgrind for Memcheck on the long-lived MCP server.
This is a bounded smoke/soak test, not proof of absence of leaks.
"""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--bin-dir', type=Path, default=Path('target/release'))
parser.add_argument('--cycles', type=int, default=1000)
parser.add_argument('--valgrind', type=Path)
parser.add_argument('--debuginfo-dir', type=Path)
parser.add_argument('--profile-log', type=Path, default=Path('/tmp/agentrun-memcheck.log'))
args = parser.parse_args()
if not 1 <= args.cycles <= 10000:
    parser.error('--cycles must be 1..10000')
binaries = args.bin_dir.resolve()
command = [str(binaries / 'agentrun-mcp')]
if args.valgrind:
    command = [str(args.valgrind.resolve()), '--leak-check=full', '--show-leak-kinds=all',
               '--errors-for-leak-kinds=definite,indirect,possible', '--error-exitcode=97',
               '--track-fds=yes', '--log-file=' + str(args.profile_log.resolve())] + command
if args.valgrind and args.debuginfo_dir:
    command.insert(1, '--extra-debuginfo-path=' + str(args.debuginfo_dir.resolve()))
with tempfile.TemporaryDirectory(prefix='agentrun-soak-') as temp:
    root = Path(temp)
    config = root / 'config' / 'agentrun'
    config.mkdir(parents=True)
    (config / 'config.json').write_text(json.dumps({
        'allowedRoots': [temp], 'profiles': {'sleep': {'command': ['/usr/bin/sleep', '30']}}
    }))
    env = dict(os.environ, XDG_STATE_HOME=str(root / 'state'), XDG_CONFIG_HOME=str(root / 'config'))
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, env=env, text=True, bufsize=1)
    serial = 0

    def send(value):
        process.stdin.write(json.dumps(value) + '\n')
        process.stdin.flush()

    def request(method, params):
        global serial
        serial += 1
        send({'jsonrpc': '2.0', 'id': serial, 'method': method, 'params': params})
        while True:
            if not select.select([process.stdout], [], [], 60)[0]:
                raise RuntimeError('MCP response timed out')
            line = process.stdout.readline()
            if not line:
                raise RuntimeError('MCP exited: ' + process.stderr.read())
            response = json.loads(line)
            if response.get('id') == serial:
                if 'error' in response:
                    raise RuntimeError(str(response))
                result = response['result']
                if result.get('isError'):
                    raise RuntimeError(str(result))
                return result

    def tool(name, arguments=None):
        return request('tools/call', {'name': name, 'arguments': arguments or {}})

    def sample(cycle):
        status = dict(line.split(':', 1) for line in Path(f'/proc/{process.pid}/status').read_text().splitlines() if ':' in line)
        children = []
        for task in Path(f'/proc/{process.pid}/task').iterdir():
            try:
                children.extend((task / 'children').read_text().split())
            except FileNotFoundError:
                pass
        fds = list(Path(f'/proc/{process.pid}/fd').iterdir())
        # Valgrind opens its own descriptors; the native MCP must own no socket.
        if not args.valgrind:
            assert not any(str(fd.readlink()).startswith('socket:') for fd in fds)
        record = {'cycles': cycle, 'rssKiB': int(status['VmRSS'].split()[0]),
                  'threads': int(status['Threads']), 'fds': len(fds), 'children': len(children)}
        print(json.dumps(record), flush=True)
        return record

    try:
        request('initialize', {'protocolVersion': '2025-11-25', 'capabilities': {},
                              'clientInfo': {'name': 'resource-check', 'version': '1'}})
        send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        tool('list_processes')
        baseline = sample(0)
        for i in range(1, args.cycles + 1):
            tool('start_process', {'id': 'probe', 'cwd': temp, 'profile': 'sleep'})
            tool('get_logs', {'id': 'probe', 'tail': 100})
            tool('restart_process', {'id': 'probe'})
            tool('stop_process', {'id': 'probe'})
            tool('clean_registry')
            if i % max(1, args.cycles // 10) == 0:
                sample(i)
        # Allow completed reaper threads to leave the process.
        time.sleep(2)
        final = sample('settled')
        assert final['children'] == 0, final
        assert final['fds'] == baseline['fds'], (baseline, final)
        assert tool('list_processes')['structuredContent']['processes'] == []
        logs = list((root / 'state' / 'agentrun' / 'logs').glob('*.log'))
        assert len(logs) <= 20, len(logs)
        print(json.dumps({'retainedLogRuns': len(logs), 'registryEmpty': True}), flush=True)
    finally:
        subprocess.run([str(binaries / 'agentrun'), 'stop', '--all'], env=env,
                       capture_output=True, timeout=60, check=False)
        process.stdin.close()
        try:
            code = process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=10)
            raise RuntimeError('MCP did not exit on stdin EOF')
    assert code == 0, (code, process.stderr.read())
