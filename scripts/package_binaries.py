#!/usr/bin/env python3
"""Package the three sibling Linux binaries, documentation and example policy."""
import argparse
import hashlib
from pathlib import Path
import tarfile
import tomllib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--bin-dir', required=True, type=Path)
parser.add_argument('--target', required=True, choices=[
    'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu'])
parser.add_argument('--out-dir', type=Path, default=Path('dist'))
args = parser.parse_args()
root = Path(__file__).resolve().parent.parent
version = tomllib.loads((root / 'Cargo.toml').read_text())['package']['version']
name = f'agentrun-{version}-{args.target}'
files = [(args.bin_dir / binary, binary, 0o755)
         for binary in ('agentrun', 'agentrun-mcp', 'agentrun-log')]
files += [(root / item, item, 0o644)
          for item in ('README.md', 'LICENSE', 'examples/config.json', '.github/workflows/ci.yml')]
files += [(path, str(path.relative_to(root)), 0o644)
          for path in sorted((root / 'docs').glob('*.md'))]
for path, _, _ in files:
    if not path.is_file() or path.is_symlink():
        raise SystemExit(f'Missing or non-regular input: {path}')
args.out_dir.mkdir(parents=True, exist_ok=True)
archive = args.out_dir / f'{name}.tar.gz'
with tarfile.open(archive, 'w:gz', format=tarfile.USTAR_FORMAT) as package:
    for path, relative, mode in files:
        info = tarfile.TarInfo(f'{name}/{relative}')
        info.size = path.stat().st_size
        info.mode = mode
        # Neutral metadata: no builder username, home path or timestamps.
        with path.open('rb') as source:
            package.addfile(info, source)
checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
archive.with_suffix(archive.suffix + '.sha256').write_text(f'{checksum}  {archive.name}\n')
print(archive)
