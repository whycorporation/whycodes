#!/usr/bin/env sh
# Install whycode from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/whycorporation/whycode/main/scripts/install.sh | sh
#
# POSIX sh on purpose: this runs on whatever shell a machine has before whycode
# is on it. Every downloaded artifact is checked against the release's
# SHA256SUMS before anything is written to the install directory.

set -eu

REPO="whycorporation/whycode"
INSTALL_DIR="${WHYCODE_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${WHYCODE_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "$1 is required but was not found"
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
    archive="whycode-${target}.tar.gz"

    if [ "$VERSION" = "latest" ]; then
        base="https://github.com/${REPO}/releases/latest/download"
    else
        base="https://github.com/${REPO}/releases/download/${VERSION}"
    fi

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT

    say "Downloading $archive"
    curl -fsSL "$base/$archive" -o "$tmp/$archive" \
        || die "could not download $base/$archive"
    curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" \
        || die "could not download the checksum file; refusing to install unverified"

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
    [ -f "$tmp/whycode" ] || die "the archive did not contain a whycode binary"

    mkdir -p "$INSTALL_DIR"
    # Install via a temporary name and rename, so an interrupted copy cannot
    # leave a half-written binary in place of a working one.
    cp "$tmp/whycode" "$INSTALL_DIR/.whycode.new"
    chmod +x "$INSTALL_DIR/.whycode.new"
    mv "$INSTALL_DIR/.whycode.new" "$INSTALL_DIR/whycode"

    say "Installed to $INSTALL_DIR/whycode"
    "$INSTALL_DIR/whycode" --version || true

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
