#!/usr/bin/env sh
# Offline fixture for scripts/update_homebrew_formula.sh.
# Feeds a fake SHA256SUMS and checks the generated formula.

set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cat > "$tmp/SHA256SUMS" <<'EOF'
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  whycodes-x86_64-unknown-linux-gnu.tar.gz
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  whycodes-aarch64-apple-darwin.tar.gz
cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  whycodes-x86_64-apple-darwin.tar.gz
dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd  whycodes-x86_64-pc-windows-msvc.zip
EOF

WHYCODES_SHA256SUMS="$tmp/SHA256SUMS" \
WHYCODES_FORMULA="$tmp/whycodes.rb" \
WHYCODES_REPO="acme/whycodes" \
    "$ROOT/scripts/update_homebrew_formula.sh" v1.2.3 >/dev/null

formula="$tmp/whycodes.rb"
[ -f "$formula" ] || { printf 'error: formula was not written\n' >&2; exit 1; }

need() {
    grep -F -q "$1" "$formula" || {
        printf 'error: formula missing %s\n' "$1" >&2
        exit 1
    }
}

need 'version "1.2.3"'
need 'class Whycodes'
need 'homepage "https://why.codes"'
need 'https://github.com/acme/whycodes/releases/download/v1.2.3/whycodes-aarch64-apple-darwin.tar.gz'
need 'https://github.com/acme/whycodes/releases/download/v1.2.3/whycodes-x86_64-apple-darwin.tar.gz'
need 'https://github.com/acme/whycodes/releases/download/v1.2.3/whycodes-x86_64-unknown-linux-gnu.tar.gz'
need 'sha256 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"'
need 'sha256 "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"'
need 'sha256 "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"'
need 'depends_on "rust" => :build'
need 'if build.head?'
need 'bin.install "whycodes"'
need 'bin.install_symlink "whycodes" => "whycode"'

# Windows zip is not a Homebrew target; must not leak into the formula.
if grep -F -q 'windows-msvc' "$formula"; then
    printf 'error: formula should not mention Windows artifacts\n' >&2
    exit 1
fi
if grep -F -q 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd' "$formula"; then
    printf 'error: formula should not include the Windows digest\n' >&2
    exit 1
fi

printf 'update_homebrew_formula: ok\n'
