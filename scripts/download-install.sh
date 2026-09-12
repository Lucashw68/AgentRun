#!/bin/sh
# The repository is an explicit argument: no account identity is embedded here.
# Keep execution in main so a truncated pipe cannot run half an installer.
usage() {
    printf '%s\n' 'Usage: sh install-agentrun.sh --repo OWNER/REPOSITORY [--version X.Y.Z] [--prefix /absolute/path] [--github-cli]' \
        'Downloads the latest stable GitHub release by default; installs in $HOME/.local/bin.' \
        'Requires Linux 6.9+, curl (or authenticated gh with --github-cli), tar and sha256sum.' \
        'No sudo, embedded account identity or token command-line argument.'
}

fail() { printf '%s\n' "AgentRun installer: $*" >&2; exit 1; }

download() {
    # -q first disables ~/.curlrc; never inherit insecure TLS or credential options.
    curl -q --fail --silent --show-error --location \
        --proto '=https' --proto-redir '=https' --tlsv1.2 \
        --connect-timeout 10 --max-time 180 --max-redirs 5 \
        --retry 2 --retry-delay 1 --max-filesize 33554432 "$@"
}

asset() {
    if [ "$github_cli" = yes ]; then
        gh release download "v$version" --repo "https://github.com/$repo" \
            --pattern "$1" --dir "$agentrun_tmp"
    else
        download --output "$agentrun_tmp/$1" "$base_url/download/v$version/$1"
    fi
}

main() {
    set -eu
    repo=''
    version=latest
    github_cli=no
    prefix="${HOME:?HOME must be set}/.local"
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --help) usage; return 0 ;;
            --github-cli) github_cli=yes; shift ;;
            --repo|--version|--prefix)
                [ "$#" -ge 2 ] || fail "Missing value for $1"
                case "$1" in --repo) repo=$2 ;; --version) version=$2 ;; --prefix) prefix=$2 ;; esac
                shift 2 ;;
            *) fail "Unknown argument: $1" ;;
        esac
    done
    printf '%s\n' "$repo" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$' ||
        fail 'Use --repo OWNER/REPOSITORY to select the trusted GitHub project.'
    case "$prefix" in /*) ;; *) fail 'Prefix must be absolute.' ;; esac
    if [ "$version" != latest ]; then
        version=${version#v}
        printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || fail 'Version must be X.Y.Z.'
    fi
    [ "$(uname -s)" = Linux ] || fail 'Linux is required.'
    case "$(uname -m)" in
        x86_64|amd64) target=x86_64-unknown-linux-musl ;;
        aarch64|arm64) target=aarch64-unknown-linux-musl ;;
        *) fail 'Supported architectures: x86_64 and ARM64.' ;;
    esac
    kernel=$(uname -r)
    major=${kernel%%.*}
    minor=${kernel#*.}; minor=${minor%%.*}; minor=${minor%%-*}
    case "$major:$minor" in *[!0-9:]*|:*|*:) fail 'Cannot determine kernel version.' ;; esac
    if [ "$major" -lt 6 ] || { [ "$major" -eq 6 ] && [ "$minor" -lt 9 ]; }; then
        fail "Linux 6.9+ required for safe group signals; found $kernel."
    fi
    if [ "$github_cli" = yes ]; then transfer=gh; else transfer=curl; fi
    for tool in "$transfer" tar sha256sum mktemp install chmod wc; do
        command -v "$tool" >/dev/null 2>&1 || fail "Missing required tool: $tool"
    done
    # Bound files produced by curl/tar, including unexpectedly large archive members.
    ulimit -f 65536
    umask 077
    agentrun_tmp=$(mktemp -d "${TMPDIR:-/tmp}/agentrun-download.XXXXXXXX")
    trap 'rm -rf -- "$agentrun_tmp"' EXIT
    trap 'exit 1' HUP INT TERM
    base_url="https://github.com/$repo/releases"
    if [ "$version" = latest ]; then
        if [ "$github_cli" = yes ]; then
            tag=$(gh release view --repo "https://github.com/$repo" --json tagName --jq .tagName) ||
                fail 'Cannot resolve the release. Check gh authentication and repository access.'
            case "$tag" in v*) version=${tag#v} ;; *) fail 'Unexpected release tag.' ;; esac
        else
            resolved=$(download --output /dev/null --write-out '%{url_effective}' "$base_url/latest") ||
                fail 'Cannot resolve the release. For a private repository, use --github-cli with authenticated gh.'
            case "$resolved" in "$base_url/tag/v"*) version=${resolved#"$base_url/tag/v"} ;;
                *) fail 'Unexpected latest-release redirect.' ;;
            esac
        fi
        printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || fail 'Unexpected release version.'
    fi
    base="agentrun-$version-$target"
    archive="$base.tar.gz"
    printf '%s\n' "Downloading AgentRun $version for $target from $repo..."
    asset "$archive.sha256" ||
        fail 'Cannot download the checksum. For a private repository, use --github-cli with authenticated gh.'
    [ "$(wc -c < "$agentrun_tmp/$archive.sha256")" -le 512 ] || fail 'Invalid checksum file.'
    [ "$(wc -l < "$agentrun_tmp/$archive.sha256")" -eq 1 ] || fail 'Invalid checksum file.'
    IFS=' ' read -r expected filename extra < "$agentrun_tmp/$archive.sha256"
    [ "${#expected}" -eq 64 ] && [ "$filename" = "$archive" ] && [ -z "$extra" ] || fail 'Invalid checksum entry.'
    case "$expected" in *[!0-9a-f]*) fail 'Invalid SHA-256.' ;; esac
    asset "$archive" ||
        fail 'Cannot download the archive.'
    actual=$(sha256sum "$agentrun_tmp/$archive"); actual=${actual%% *}
    [ "$actual" = "$expected" ] || fail 'SHA-256 mismatch; installation was not changed.'

    # Extract only allowlisted regular files to fixed destinations through stdout.
    # Archive paths, links, devices and unrelated members are never materialized.
    tar -tzf "$agentrun_tmp/$archive" > "$agentrun_tmp/members"
    mkdir "$agentrun_tmp/bundle"
    members='agentrun agentrun-mcp agentrun-log install.sh'
    # Keep installation of releases preceding the setup utility supported.
    if grep -Fq "$base/agentrun-setup" "$agentrun_tmp/members"; then
        members="$members agentrun-setup"
    fi
    for member in $members; do
        entry="$base/$member"
        [ "$(grep -Fxc "$entry" "$agentrun_tmp/members")" -eq 1 ] || fail "Missing or duplicate archive member: $member"
        details=$(LC_ALL=C tar -tvzf "$agentrun_tmp/$archive" "$entry")
        case "$details" in -*) ;; *) fail "Not a regular archive file: $member" ;; esac
        tar -xzOf "$agentrun_tmp/$archive" "$entry" > "$agentrun_tmp/bundle/$member"
        chmod 755 "$agentrun_tmp/bundle/$member"
    done
    printf '%s\n' 'SHA-256 verified. Installing...'
    sh "$agentrun_tmp/bundle/install.sh" --prefix "$prefix" </dev/null
    printf '%s\n' "Release and documentation: $base_url/tag/v$version"
}

main "$@"
