#!/usr/bin/env python3
"""Exercise packaged setup in isolated homes; optional real Codex CLI integration."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--archive', type=Path, required=True)
parser.add_argument('--codex', type=Path, help='also exercise this real Codex CLI in a temporary CODEX_HOME')
args = parser.parse_args()
REAL_CODEX = args.codex.resolve() if args.codex else None
ROOT = tempfile.TemporaryDirectory(prefix='agentrun-setup-test-')
root = Path(ROOT.name)
with tarfile.open(args.archive) as archive:
    archive.extractall(root, filter='data')
bundle = next(root.iterdir())
prefix = root / 'prefix with spaces'
subprocess.run(['sh', str(bundle / 'install.sh'), '--prefix', str(prefix)], check=True, capture_output=True)
SETUP = prefix / 'bin/agentrun-setup'
EXPECTED = json.loads((bundle / 'examples/config.json').read_text())
FAKE_CODEX = '''#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
home=Path(os.environ['CODEX_HOME'])
state=home/'fixture.json'
args=sys.argv[1:]
with (home/'calls').open('a') as log: log.write(json.dumps(args)+'\\n')
if args==['mcp','list','--json']:
 print(state.read_text() if state.exists() else '[]'); sys.exit(0)
assert args[:4]==['mcp','add','agentrun','--'],args
assert len(args)==5,args
if os.environ.get('CODEX_FAIL'):
 print('secret-example-do-not-print',file=sys.stderr); sys.exit(1)
state.write_text(json.dumps([{'name':'agentrun','enabled':True,
 'transport':{'type':'stdio','command':args[4],'args':[],'env':None,'env_vars':[],'cwd':None}}]))
'''


class SetupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=root)
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        commands = self.home / 'commands'
        commands.mkdir()
        codex = commands / 'codex'
        codex.write_text(FAKE_CODEX)
        codex.chmod(0o755)
        self.env = dict(os.environ, HOME=str(self.home), XDG_CONFIG_HOME='',
                        CODEX_HOME=str(self.home / '.codex'),
                        PATH=str(commands) + os.pathsep + os.environ['PATH'])
        self.policy = self.home / '.config/agentrun/config.json'
        self.codex_home = self.home / '.codex'
        self.instructions = self.codex_home / 'AGENTS.md'

    def run_setup(self, target='codex', success=True, extra=(), **env):
        result = subprocess.run([str(SETUP), target, *extra], env=dict(self.env, **env),
                                capture_output=True, text=True, timeout=40)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def seed(self, path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def test_default_policy_and_idempotent_registration(self):
        self.run_setup()
        self.assertEqual(json.loads(self.policy.read_text()), EXPECTED)
        self.assertEqual(len(EXPECTED['profiles']), 36)
        self.assertEqual(self.policy.stat().st_mode & 0o777, 0o600)
        before = self.instructions.read_bytes()
        modified = self.instructions.stat().st_mtime_ns
        self.run_setup()
        self.assertEqual(self.instructions.read_bytes(), before)
        self.assertEqual(self.instructions.stat().st_mtime_ns, modified)
        calls = [json.loads(line) for line in (self.codex_home / 'calls').read_text().splitlines()]
        self.assertEqual(sum(call[1] == 'add' for call in calls), 1)
        self.assertIn(str(prefix / 'bin/agentrun-mcp'), str(calls))

    def test_config_only_needs_no_codex_or_binary_directory(self):
        self.run_setup('config', extra=('--bin-dir', '/nonexistent'), CODEX_HOME='irrelevant-relative')
        self.assertFalse(self.codex_home.exists())
        self.seed(self.policy, '{"allowedRoots":[],"profiles":{}}\n')
        self.run_setup('config')
        self.assertEqual(self.policy.read_text(), '{"allowedRoots":[],"profiles":{}}\n')

    def test_missing_codex_fails_before_writing_configuration(self):
        commands = self.home / 'only-python'
        commands.mkdir()
        (commands / 'python3').symlink_to(shutil.which('python3'))
        self.run_setup(success=False, PATH=str(commands))
        self.assertFalse(self.policy.exists())
        self.assertFalse(self.codex_home.exists())

    def test_preserves_policy_and_existing_instructions(self):
        original = '# Team instructions\n\nDo not modify unrelated files.'
        self.seed(self.instructions, original)
        self.seed(self.policy, '{"allowedRoots":[],"profiles":{}}\n')
        self.run_setup()
        self.assertTrue(self.instructions.read_text().startswith(original + '\n\n'))
        self.assertEqual(json.loads(self.policy.read_text())['profiles'], {})
        self.assertEqual(self.instructions.read_text().count('<!-- agentrun:instructions:start -->'), 1)
        # Only the managed block is refreshed; edits outside it are preserved.
        self.instructions.write_text(self.instructions.read_text().replace('Use distinct IDs', 'Old recommendation') + '\nUser suffix\n')
        self.run_setup()
        self.assertNotIn('Old recommendation', self.instructions.read_text())
        self.assertTrue(self.instructions.read_text().endswith('\nUser suffix\n'))

    def test_override_and_custom_paths(self):
        custom = self.home / 'custom codex'
        override = custom / 'AGENTS.override.md'
        self.seed(override, '# Global override\n')
        self.seed(custom / 'AGENTS.md', 'leave untouched')
        self.run_setup(CODEX_HOME=str(custom), XDG_CONFIG_HOME=str(self.home / 'custom config'))
        self.assertIn('agentrun:instructions:start', override.read_text())
        self.assertEqual((custom / 'AGENTS.md').read_text(), 'leave untouched')
        self.assertTrue((self.home / 'custom config/agentrun/config.json').is_file())
        self.assertFalse(self.policy.exists())

    def test_empty_override_uses_agents(self):
        self.seed(self.codex_home / 'AGENTS.override.md', '\n')
        self.run_setup()
        self.assertTrue(self.instructions.exists())
        self.assertEqual((self.codex_home / 'AGENTS.override.md').read_text(), '\n')

    def test_relative_xdg_falls_back_and_relative_codex_is_refused(self):
        self.run_setup('config', XDG_CONFIG_HOME='relative')
        self.assertTrue(self.policy.exists())
        self.run_setup(success=False, CODEX_HOME='relative')
        self.assertFalse(self.instructions.exists())

    def test_existing_registration_conflict_is_preserved(self):
        value = '[{"name":"agentrun","transport":{"type":"streamable_http","url":"https://example.invalid"}}]'
        self.seed(self.codex_home / 'fixture.json', value)
        self.run_setup(success=False)
        self.assertEqual((self.codex_home / 'fixture.json').read_text(), value)
        self.assertFalse(self.policy.exists())
        self.assertFalse(self.instructions.exists())

    def test_registration_failure_is_reported_without_secrets_and_can_retry(self):
        result = self.run_setup(success=False, CODEX_FAIL='1')
        self.assertNotIn('secret-example-do-not-print', result.stdout + result.stderr)
        self.assertTrue(self.policy.exists())
        self.assertFalse(self.instructions.exists())
        self.run_setup()
        self.assertTrue(self.instructions.exists())

    def test_malformed_markers_fail_before_mutation(self):
        content = '# Keep\n<!-- agentrun:instructions:start -->\ntruncated'
        self.seed(self.instructions, content)
        self.run_setup(success=False)
        self.assertEqual(self.instructions.read_text(), content)
        self.assertFalse(self.policy.exists())
        self.assertFalse((self.codex_home / 'fixture.json').exists())

    def test_symlinks_and_hardlinks_are_rejected(self):
        sentinel = self.home / 'sentinel'
        sentinel.write_text('preserve')
        for target in [self.policy, self.instructions, self.codex_home / 'AGENTS.override.md', self.codex_home / 'config.toml']:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.symlink_to(sentinel)
            self.run_setup(success=False)
            target.unlink()
            self.assertEqual(sentinel.read_text(), 'preserve')
        os.link(sentinel, self.instructions)
        self.run_setup(success=False)
        self.assertEqual(sentinel.read_text(), 'preserve')

    def test_redirected_parent_and_fifo_are_refused(self):
        elsewhere = self.home / 'elsewhere'
        elsewhere.mkdir()
        (self.home / '.config').symlink_to(elsewhere, target_is_directory=True)
        self.run_setup('config', success=False)
        self.assertEqual(list(elsewhere.iterdir()), [])
        (self.home / '.config').unlink()
        self.policy.parent.mkdir(parents=True)
        os.mkfifo(self.policy)
        self.run_setup('config', success=False)

    def test_invalid_existing_json_and_missing_binary_are_preserved(self):
        self.seed(self.policy, '{bad json')
        self.run_setup(success=False)
        self.assertEqual(self.policy.read_text(), '{bad json')
        self.policy.unlink()
        self.run_setup(success=False, extra=('--bin-dir', '/nonexistent'))
        self.assertFalse(self.policy.exists())

    def test_concurrent_setup_is_refused_without_mutation(self):
        self.policy.parent.mkdir(parents=True)
        with (self.policy.parent / '.setup.lock').open('w') as handle:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.run_setup(success=False)
        self.assertFalse(self.policy.exists())
        self.run_setup()

    @unittest.skipUnless(REAL_CODEX, 'pass --codex to exercise the installed Codex CLI')
    def test_real_codex_registration_preserves_unrelated_configuration(self):
        (self.home / 'commands/codex').unlink()
        (self.home / 'commands/codex').symlink_to(REAL_CODEX)
        self.seed(self.codex_home / 'config.toml', '# Keep comment\nmodel = "example-model"\n\n[mcp_servers.other]\ncommand = "/usr/bin/true"\n')
        self.run_setup()
        first = (self.codex_home / 'config.toml').read_bytes()
        self.run_setup()
        self.assertEqual((self.codex_home / 'config.toml').read_bytes(), first)
        self.assertIn(b'# Keep comment', first)
        self.assertIn(b'example-model', first)
        self.assertIn(b'[mcp_servers.other]', first)
        self.assertIn(b'[mcp_servers.agentrun]', first)


try:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(SetupTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful():
        raise SystemExit(1)
finally:
    ROOT.cleanup()
