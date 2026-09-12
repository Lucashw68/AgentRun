#!/bin/sh
# Offline user installation from an extracted AgentRun binary archive.
set -eu

usage() {
    printf '%s\n' 'Usage: sh install.sh [--prefix /absolute/path]' \
        'Default prefix: $HOME/.local. Installs the three binaries and agentrun-setup in PREFIX/bin.' \
        'No download, privilege escalation, shell configuration or policy changes.'
}
if [ "${1:-}" = '--help' ]; then usage; exit 0; fi
prefix="${HOME:?HOME must be set}/.local"
if [ "$#" -gt 0 ]; then
    if [ "$#" -ne 2 ] || [ "$1" != '--prefix' ]; then usage >&2; exit 2; fi
    prefix=$2
fi
case "$prefix" in /*) ;; *) printf '%s\n' 'Prefix must be absolute.' >&2; exit 2 ;; esac
if [ "$(uname -s)" != Linux ]; then
    printf '%s\n' 'AgentRun requires Linux.' >&2; exit 1
fi
kernel=$(uname -r)
major=${kernel%%.*}
minor=${kernel#*.}; minor=${minor%%.*}; minor=${minor%%-*}
case "$major:$minor" in *[!0-9:]*|:*|*:) printf '%s\n' 'Cannot determine kernel version.' >&2; exit 1 ;; esac
if [ "$major" -lt 6 ] || { [ "$major" -eq 6 ] && [ "$minor" -lt 9 ]; }; then
    printf '%s\n' "Linux 6.9+ required for safe process group signals; found $kernel." >&2
    exit 1
fi
bundle=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
for binary in agentrun agentrun-mcp agentrun-log agentrun-setup; do
    if [ ! -f "$bundle/$binary" ] || [ -L "$bundle/$binary" ] || [ ! -x "$bundle/$binary" ]; then
        printf '%s\n' "Missing executable in archive: $binary" >&2; exit 1
    fi
    if [ -L "$prefix/bin/$binary" ] || [ -d "$prefix/bin/$binary" ]; then
        printf '%s\n' "Refusing symlink or directory destination: $prefix/bin/$binary" >&2; exit 1
    fi
done
# Detect an incompatible CPU or executable format before changing the prefix.
"$bundle/agentrun" --version
mkdir -p "$prefix/bin"
for binary in agentrun agentrun-mcp agentrun-log agentrun-setup; do
    install -m 755 "$bundle/$binary" "$prefix/bin/$binary"
done
printf '%s\n' "Installed AgentRun in $prefix/bin." \
    'Add that directory to PATH in your shell.' \
    'Then run: agentrun-setup codex (requires Python 3 and the Codex CLI).' \
    'For other agents: agentrun-setup config, then register the stdio MCP in your client.' \
    'See docs/install.md and docs/agents.md in the archive.'
