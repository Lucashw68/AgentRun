#!/bin/sh
# Executed as an unprivileged user in a disposable, network-disabled container.
set -eu
export XDG_STATE_HOME=/tmp/agentrun-state
export XDG_CONFIG_HOME=/tmp/agentrun-config
sh /bundle/install.sh --prefix /tmp/agentrun-install
export PATH="/tmp/agentrun-install/bin:$PATH"
agentrun --version
agentrun list --json
trap 'agentrun stop --all >/dev/null 2>&1 || true' EXIT
agentrun start distro-smoke -- sleep 30
agentrun status distro-smoke --json > /tmp/agentrun-before.json
grep -q '"status":"running"' /tmp/agentrun-before.json
agentrun logs distro-smoke --tail 10
agentrun restart distro-smoke
agentrun status distro-smoke --json > /tmp/agentrun-after.json
grep -q '"status":"running"' /tmp/agentrun-after.json
agentrun stop distro-smoke
agentrun clean --json
agentrun list --json > /tmp/agentrun-empty.json
grep -q '"processes":\[\]' /tmp/agentrun-empty.json
# Exercise the actual stdio server in each distribution, without a network.
{
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"distro-check","version":"1"}}}'
    sleep 1
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
        '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_processes","arguments":{}}}'
    sleep 1
} | agentrun-mcp > /tmp/agentrun-mcp.json
grep -q '"processes":\[\]' /tmp/agentrun-mcp.json
printf '%s\n' 'Distribution installation, CLI lifecycle and MCP stdio: OK'
