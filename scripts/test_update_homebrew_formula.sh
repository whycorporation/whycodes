#!/usr/bin/env sh
# Offline fixture for scripts/update_homebrew_formula.sh.
# Feeds a fake SHA256SUMS and checks the generated formula.

set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cat > "$tmp/SHA256SUMS" <<'EOF'
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  whycode-x86_64-unknown-linux-gnu.tar.gz
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  whycode-aarch64-apple-darwin.tar.gz
cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  whycode-x86_64-apple-darwin.tar.gz
dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd  whycode-x86_64-pc-windows-msvc.zip
EOF

WHYCODE_SHA256SUMS="$tmp/SHA256SUMS" \
WHYCODE_FORMULA="$tmp/whycode.rb" \
WHYCODE_REPO="acme/whycode" \
    "$ROOT/scripts/update_homebrew_formula.sh" v1.2.3 >/dev/null

formula="$tmp/whycode.rb"
[ -f "$formula" ] || { printf 'error: formula was not written\n' >&2; exit 1; }

need() {
    grep -F -q "$1" "$formula" || {
        printf 'error: formula missing %s\n' "$1" >&2
        exit 1
    }
}

need 'version "1.2.3"'
need 'homepage "https://github.com/acme/whycode"'
need 'https://github.com/acme/whycode/releases/download/v1.2.3/whycode-aarch64-apple-darwin.tar.gz'
need 'https://github.com/acme/whycode/releases/download/v1.2.3/whycode-x86_64-apple-darwin.tar.gz'
need 'https://github.com/acme/whycode/releases/download/v1.2.3/whycode-x86_64-unknown-linux-gnu.tar.gz'
need 'sha256 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"'
need 'sha256 "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"'
need 'sha256 "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"'
need 'depends_on "rust" => :build'
need 'if build.head?'
need 'bin.install "whycode"'

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
