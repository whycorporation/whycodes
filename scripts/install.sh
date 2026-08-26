#!/usr/bin/env sh
# Install whycodes from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/whycorporation/whycodes/main/scripts/install.sh | sh
#
# Public repos: no auth required.
# Private repos: set GITHUB_TOKEN or GH_TOKEN (classic or fine-grained with
# Contents read). Browser download URLs 404 on private releases; this script
# falls back to the GitHub API asset endpoint when a token is present.
#
# POSIX sh on purpose: this runs on whatever shell a machine has before whycodes
# is on it. Every downloaded artifact is checked against the release's
# SHA256SUMS before anything is written to the install directory.

set -eu

REPO="whycorporation/whycodes"
INSTALL_DIR="${WHYCODES_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${WHYCODES_VERSION:-latest}"
# Prefer GITHUB_TOKEN; accept GH_TOKEN (gh CLI).
TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "$1 is required but was not found"
}

# curl with optional Bearer auth.
curl_auth() {
    if [ -n "$TOKEN" ]; then
        curl -fsSL -H "Authorization: Bearer ${TOKEN}" "$@"
    else
        curl -fsSL "$@"
    fi
}

# Download a named asset. Tries public release URL first; on failure with a
# token, uses the releases API asset id + Accept: application/octet-stream.
download_asset() {
    name="$1"
    out="$2"
    tag_or_latest="$3"

    if [ "$tag_or_latest" = "latest" ]; then
        public_url="https://github.com/${REPO}/releases/latest/download/${name}"
        api_release_url="https://api.github.com/repos/${REPO}/releases/latest"
    else
        public_url="https://github.com/${REPO}/releases/download/${tag_or_latest}/${name}"
        api_release_url="https://api.github.com/repos/${REPO}/releases/tags/${tag_or_latest}"
    fi

    if curl_auth "$public_url" -o "$out" 2>/dev/null; then
        return 0
    fi

    [ -n "$TOKEN" ] || die "could not download $public_url
hint: if the repository is private, export GITHUB_TOKEN or GH_TOKEN and retry"

    # Resolve asset id from the release JSON, then stream the blob.
    need python3
    asset_id="$(
        curl_auth -H "Accept: application/vnd.github+json" "$api_release_url" \
            | python3 -c "
import json,sys
name=sys.argv[1]
rel=json.load(sys.stdin)
for a in rel.get('assets') or []:
    if a.get('name')==name:
        print(a['id']); raise SystemExit(0)
raise SystemExit('asset not found: '+name)
" "$name"
    )" || die "could not resolve asset id for $name via API"

    curl_auth \
        -H "Accept: application/octet-stream" \
        "https://api.github.com/repos/${REPO}/releases/assets/${asset_id}" \
        -o "$out" \
        || die "could not download asset $name via API"
}

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Linux)  os_part="unknown-linux-gnu" ;;
        Darwin) os_part="apple-darwin" ;;
        *) die "unsupported operating system: $os" ;;
    esac
    case "$arch" in
        x86_64|amd64) arch_part="x86_64" ;;
        arm64|aarch64) arch_part="aarch64" ;;
        *) die "unsupported architecture: $arch" ;;
    esac
    # Only Apple silicon has an aarch64 build; Linux ships x86_64 only.
    if [ "$os_part" = "unknown-linux-gnu" ] && [ "$arch_part" != "x86_64" ]; then
        die "no prebuilt binary for $arch on Linux — build from source with 'cargo build --release'"
    fi
    printf '%s-%s' "$arch_part" "$os_part"
}

# sha256 has a different name on almost every system.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        die "neither sha256sum nor shasum is available; cannot verify the download"
    fi
}

main() {
    need curl
    need tar

    target="$(detect_target)"
    archive="whycodes-${target}.tar.gz"

    # Normalize optional leading v for tag URLs.
    case "$VERSION" in
        latest) tag_arg="latest" ;;
        v*) tag_arg="$VERSION" ;;
        *) tag_arg="v$VERSION" ;;
    esac

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT

    say "Downloading $archive"
    download_asset "$archive" "$tmp/$archive" "$tag_arg"
    download_asset "SHA256SUMS" "$tmp/SHA256SUMS" "$tag_arg"

    # Lines are either "hex  name" or "hex *name" (binary mode from sha256sum).
    expected="$(grep -E "[[:space:]](\\*)?${archive}\$" "$tmp/SHA256SUMS" | awk '{print $1}' | head -n1)"
    [ -n "$expected" ] || die "$archive is not listed in SHA256SUMS"
    actual="$(sha256_of "$tmp/$archive")"
    if [ "$expected" != "$actual" ]; then
        die "checksum mismatch for $archive
  expected $expected
  actual   $actual
Nothing was installed."
    fi
    say "Checksum verified"

    tar -xzf "$tmp/$archive" -C "$tmp"
    [ -f "$tmp/whycodes" ] || die "the archive did not contain a whycodes binary"

    mkdir -p "$INSTALL_DIR"
    # Install via a temporary name and rename, so an interrupted copy cannot
    # leave a half-written binary in place of a working one.
    cp "$tmp/whycodes" "$INSTALL_DIR/.whycodes.new"
    chmod +x "$INSTALL_DIR/.whycodes.new"
    mv "$INSTALL_DIR/.whycodes.new" "$INSTALL_DIR/whycodes"
    ln -sfn whycodes "$INSTALL_DIR/whycode"

    say "Installed to $INSTALL_DIR/whycodes"
    "$INSTALL_DIR/whycodes" --version || true

    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            say ""
            say "$INSTALL_DIR is not on your PATH. Add it:"
            say "    export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac
}

main "$@"
