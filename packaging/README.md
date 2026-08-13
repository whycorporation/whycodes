# Packaging (partial)

Distribution beyond the install scripts. This is intentionally thin; full
package-manager automation (auto-bump CI, bottles, winget, AUR, Nix) comes
later.

## Homebrew

Formula lives in-repo at [`Formula/whycode.rb`](../Formula/whycode.rb) so a
separate `homebrew-whycode` tap is not required yet.

```bash
# Source build from main (needs Rust via brew)
brew tap whycorporation/whycode https://github.com/whycorporation/whycode
brew install --HEAD whycode

# Or one-shot without a persistent tap
brew install --HEAD --formula \
  https://raw.githubusercontent.com/whycorporation/whycode/main/Formula/whycode.rb
```

After a tagged release publishes binaries + `SHA256SUMS`:

```bash
scripts/update_homebrew_formula.sh v0.1.0
# review Formula/whycode.rb, commit, push
brew update && brew upgrade whycode
```

Artifact names must stay aligned with `.github/workflows/release.yml`,
`scripts/install.sh`, and `crates/cli/src/upgrade.rs`.

### Later

- Dedicated tap repo (`whycorporation/homebrew-whycode`) if formula noise in
  the main tree becomes annoying
- Auto-run `update_homebrew_formula.sh` from `release.yml`
- Linux aarch64 prebuild (formula currently documents intel-only binaries)
- winget / scoop, AUR, Nix flakes

## Install scripts (already production)

See `scripts/install.sh`, `install.ps1`, and `whycode upgrade`. Those remain
the primary path until package managers are first-class.

The scripts verify the download against the release `SHA256SUMS`. They do
not modify `PATH`; they print the install directory if it is not already on
it. `WHYCODE_INSTALL_DIR` overrides the location.

`scripts/uninstall.sh` / `uninstall.ps1` remove the binary. Add `--purge` /
`-Purge` to delete config and session data as well.

The binaries are unsigned, so macOS Gatekeeper and Windows SmartScreen will
warn on first run. `whycode upgrade` replaces the running binary with the
newest release (checksum verified) and leaves the existing one in place if
anything fails.

From source, the binary lands at `target/release/whycode`. Optional extras
(Unicode mermaid + extra highlight languages, ~+1.7 MB):

```bash
cargo build --release -p whycode-cli --features full
```
