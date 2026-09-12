#!/usr/bin/env python3
"""Check Linux ELF architecture and reject dynamic runtime dependencies."""
import argparse
from pathlib import Path
import struct


def verify(path, target):
    data = path.read_bytes()
    machine = 183 if target.startswith('aarch64-') else 62
    if len(data) < 64 or data[:6] != b'\x7fELF\x02\x01':
        raise ValueError(f'{path}: expected a little-endian ELF64 executable')
    if struct.unpack_from('<H', data, 18)[0] != machine:
        raise ValueError(f'{path}: wrong CPU architecture for {target}')
    if not target.endswith('-musl'):
        return
    offset = struct.unpack_from('<Q', data, 32)[0]
    size, count = struct.unpack_from('<HH', data, 54)
    if size < 56 or not count or offset + size * count > len(data):
        raise ValueError(f'{path}: invalid program headers')
    for i in range(count):
        kind, _, start, _, _, length, _, _ = struct.unpack_from('<IIQQQQQQ', data, offset + i * size)
        if kind == 3:  # PT_INTERP
            raise ValueError(f'{path}: dynamic interpreter found')
        if kind == 2:  # PT_DYNAMIC: a static PIE may still have relocations.
            if start + length > len(data) or length % 16:
                raise ValueError(f'{path}: invalid dynamic segment')
            for entry in range(start, start + length, 16):
                tag = struct.unpack_from('<q', data, entry)[0]
                if tag == 0:
                    break
                if tag == 1:  # DT_NEEDED
                    raise ValueError(f'{path}: shared library dependency found')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin-dir', type=Path, required=True)
    parser.add_argument('--target', choices=['x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'], required=True)
    args = parser.parse_args()
    for binary in ('agentrun', 'agentrun-mcp', 'agentrun-log'):
        verify(args.bin_dir / binary, args.target)
    print(f'{args.target}: three static ELF binaries verified')
