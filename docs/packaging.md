# Packaging

Distribution beyond the install scripts. Dedicated package-manager repos
(winget, AUR, Nix) stay later.

## Homebrew

Formula lives in-repo at [`Formula/whycodes.rb`](../Formula/whycodes.rb) so a
separate `homebrew-whycodes` tap is not required yet. Tagged releases install
prebuilt binaries (no Rust). `--HEAD` still compiles from `main`.

```bash
brew tap whycorporation/whycode https://github.com/whycorporation/whycode
brew install whycodes

# Tip of main (needs Rust via brew)
brew install --HEAD whycodes

# One-shot without a persistent tap
brew install --formula \
  https://raw.githubusercontent.com/whycorporation/whycode/main/Formula/whycodes.rb
```

`release.yml` rewrites the formula from the published `SHA256SUMS` and commits
it to `main` after each `v*` tag. Manual backfill:

```bash
scripts/update_homebrew_formula.sh v0.1.0
# review Formula/whycodes.rb, commit, push
```

Artifact names must stay aligned with `.github/workflows/release.yml`,
`scripts/install.sh`, and `crates/cli/src/upgrade.rs`.

If `main` is protected against `GITHUB_TOKEN` pushes, the Homebrew job will
fail after a successful release; bump the formula locally with the script
above or allow that bot push.

### Later

- Dedicated tap repo (`whycorporation/homebrew-whycodes`) if formula noise in
  the main tree becomes annoying
- Linux aarch64 prebuild (formula currently documents intel-only binaries)
- winget / scoop, AUR, Nix flakes

## Install scripts (already production)

See `scripts/install.sh`, `install.ps1`, and `whycodes upgrade`. Those remain
the primary path until package managers are first-class.

The scripts verify the download against the release `SHA256SUMS`. They do
not modify `PATH`; they print the install directory if it is not already on
it. `WHYCODES_INSTALL_DIR` overrides the location.

`scripts/uninstall.sh` / `uninstall.ps1` remove the binary. Add `--purge` /
`-Purge` to delete config and session data as well.

The binaries are unsigned, so macOS Gatekeeper and Windows SmartScreen will
warn on first run. `whycodes upgrade` replaces the running binary with the
newest release (checksum verified) and leaves the existing one in place if
anything fails.

From source, the binary lands at `target/release/whycodes`. Optional extras
(Unicode mermaid + extra highlight languages, ~+1.7 MB):

```bash
cargo build --release -p whycodes-cli --features full
```
