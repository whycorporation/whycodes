#!/usr/bin/env sh
# Offline fixture for scripts/update_homebrew_formula.sh.
# Feeds a fake SHA256SUMS and checks the generated formula.

set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

need() {
    formula="$1"
    needle="$2"
    grep -F -q "$needle" "$formula" || {
        printf 'error: formula missing %s\n' "$needle" >&2
        exit 1
    }
}

forbid() {
    formula="$1"
    needle="$2"
    msg="$3"
    if grep -F -q "$needle" "$formula"; then
        printf 'error: %s\n' "$msg" >&2
        exit 1
    fi
}

assert_common() {
    formula="$1"
    need "$formula" 'class Whycodes'
    need "$formula" 'homepage "https://why.codes"'
    need "$formula" 'depends_on "rust" => :build'
    need "$formula" 'if build.head?'
    need "$formula" 'system "cargo", "install", "--locked"'
    need "$formula" 'generate_completions_from_executable(bin/"whycodes", "completions")'
    need "$formula" 'livecheck do'
    need "$formula" 'strategy :github_latest'
    need "$formula" 'brew upgrade whycodes'
    need "$formula" 'assert_match "whycodes", shell_output("#{bin}/whycodes --version")'
    need "$formula" 'assert_match "whycodes", shell_output("#{bin}/whycodes completions bash")'
    forbid "$formula" 'windows-msvc' 'formula should not mention Windows artifacts'
    if grep -E -q 'bin.install_symlink.*whycode' "$formula"; then
        printf 'error: formula should not install a whycode alias\n' >&2
        exit 1
    fi
}

# Current artifact names (post-rebrand).
cat > "$tmp/SHA256SUMS" <<'EOF'
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  whycodes-x86_64-unknown-linux-gnu.tar.gz
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  whycodes-aarch64-apple-darwin.tar.gz
cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  whycodes-x86_64-apple-darwin.tar.gz
dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd  whycodes-x86_64-pc-windows-msvc.zip
EOF

WHYCODES_SHA256SUMS="$tmp/SHA256SUMS" \
WHYCODES_FORMULA="$tmp/whycodes.rb" \
WHYCODES_REPO="acme/whycodes" \
    sh "$ROOT/scripts/update_homebrew_formula.sh" v1.2.3 >/dev/null

formula="$tmp/whycodes.rb"
[ -f "$formula" ] || { printf 'error: formula was not written\n' >&2; exit 1; }

assert_common "$formula"
need "$formula" 'version "1.2.3"'
need "$formula" 'https://github.com/acme/whycodes/releases/download/v1.2.3/whycodes-aarch64-apple-darwin.tar.gz'
need "$formula" 'https://github.com/acme/whycodes/releases/download/v1.2.3/whycodes-x86_64-apple-darwin.tar.gz'
need "$formula" 'https://github.com/acme/whycodes/releases/download/v1.2.3/whycodes-x86_64-unknown-linux-gnu.tar.gz'
need "$formula" 'sha256 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"'
need "$formula" 'sha256 "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"'
need "$formula" 'sha256 "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"'
need "$formula" 'bin.install "whycodes"'
forbid "$formula" 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd' \
    'formula should not include the Windows digest'
# Modern artifacts must not fall back to the pre-rebrand archive names.
if grep -E -q 'whycode-(aarch64|x86_64)-' "$formula"; then
    printf 'error: modern SHA256SUMS should not emit whycode-* archive URLs\n' >&2
    exit 1
fi

# v0.1.0 shipped as whycode-* with a whycode binary inside the tarball.
cat > "$tmp/SHA256SUMS.legacy" <<'EOF'
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  whycode-x86_64-unknown-linux-gnu.tar.gz
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  whycode-aarch64-apple-darwin.tar.gz
cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  whycode-x86_64-apple-darwin.tar.gz
dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd  whycode-x86_64-pc-windows-msvc.zip
EOF

WHYCODES_SHA256SUMS="$tmp/SHA256SUMS.legacy" \
WHYCODES_FORMULA="$tmp/whycodes-legacy.rb" \
WHYCODES_REPO="acme/whycodes" \
    sh "$ROOT/scripts/update_homebrew_formula.sh" v0.1.0 >/dev/null

legacy="$tmp/whycodes-legacy.rb"
assert_common "$legacy"
need "$legacy" 'version "0.1.0"'
need "$legacy" 'https://github.com/acme/whycodes/releases/download/v0.1.0/whycode-aarch64-apple-darwin.tar.gz'
need "$legacy" 'bin.install "whycode" => "whycodes"'

printf 'update_homebrew_formula: ok\n'
