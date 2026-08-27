# Packaging

Distribution beyond the install scripts. Dedicated package-manager repos
(winget, AUR, Nix) stay later.

## Homebrew

Formula lives in-repo at [`Formula/whycodes.rb`](../Formula/whycodes.rb) so a
separate `homebrew-whycodes` tap is not required yet. Tagged releases install
prebuilt binaries (no Rust). `--HEAD` still compiles from `main`.

macOS is the primary Homebrew audience (Apple silicon + Intel). Linuxbrew
x86_64 is also wired; Linux aarch64 still needs a prebuild.

```bash
brew tap whycorporation/whycodes https://github.com/whycorporation/whycodes
brew install whycodes
brew upgrade whycodes

# Tip of main (needs Rust via brew)
brew install --HEAD whycodes

# One-shot without a persistent tap
brew install --formula \
  https://raw.githubusercontent.com/whycorporation/whycodes/main/Formula/whycodes.rb
```

`brew install` generates bash/zsh/fish completions from
`whycodes completions`. Update with `brew upgrade whycodes` — `whycodes
upgrade` refuses to overwrite a Homebrew Cellar/prefix binary.

The formula is a **binary** bottle-style install (download the GitHub release
tarball, checksum it, drop `whycodes` into `bin`). It is not a Homebrew-core
bottle. Unsigned macOS binaries still trip Gatekeeper on first run.

`release.yml` rewrites the formula from the published `SHA256SUMS` and commits
it to `main` after each `v*` tag. Manual backfill:

```bash
scripts/update_homebrew_formula.sh v0.1.0
# review Formula/whycodes.rb, commit, push
```

Artifact names must stay aligned with `.github/workflows/release.yml`,
`scripts/install.sh`, and `crates/cli/src/upgrade.rs`. The updater prefers
`whycodes-<target>.tar.gz`. **v0.1.0** shipped as `whycode-*` with a `whycode`
binary in the archive; the formula still installs that as `whycodes`.

macOS release builds set `MACOSX_DEPLOYMENT_TARGET` (11.0 on arm64, 10.15 on
Intel) so the prebuilt does not require the runner's latest SDK.

If `main` is protected against `GITHUB_TOKEN` pushes, the Homebrew job will
fail after a successful release; bump the formula locally with the script
above or allow that bot push.

### Later

- Dedicated tap repo (`whycorporation/homebrew-whycodes`) if formula noise in
  the main tree becomes annoying
- Linux aarch64 prebuild (formula currently documents intel-only binaries)
- Code signing / notarization (Gatekeeper warning stays until then)
- winget / scoop, AUR, Nix flakes

## Install scripts (already production)

See `scripts/install.sh`, `install.ps1`, and `whycodes upgrade`. Those remain
the primary path until package managers are first-class.

The scripts verify the download against the release `SHA256SUMS`. They do
not modify `PATH`; they print the install directory if it is not already on
it. `WHYCODES_INSTALL_DIR` overrides the location. Installers and Homebrew
ship only the `whycodes` binary. Public releases need no GitHub token;
`GITHUB_TOKEN` / `GH_TOKEN` remain an optional fallback if you point the
scripts at a private fork.

`scripts/uninstall.sh` / `uninstall.ps1` remove the binary. Add `--purge` /
`-Purge` to delete config and session data as well. Homebrew users should
`brew uninstall whycodes` instead.

The binaries are unsigned, so macOS Gatekeeper and Windows SmartScreen will
warn on first run. `whycodes upgrade` replaces the running binary with the
newest release (checksum verified) and leaves the existing one in place if
anything fails — except Homebrew installs, which it will not touch.

From source, the binary lands at `target/release/whycodes`. Optional extras
(Unicode mermaid + extra highlight languages, ~+1.7 MB):

```bash
cargo build --release -p whycodes-cli --features full
```
