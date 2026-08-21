# Plan — Distribution and self-update

**Status:** implemented (assets cut) · **Priority:** **last**  
**Residual:** repo visibility=public, Windows install.ps1 smoke.

## Problem

The only way to get whycode is:

```bash
git clone … && cargo build --release
```

That requires a Rust toolchain and several minutes of compilation. It excludes
everyone who wants to try the tool rather than work on it.

`whycode upgrade` compounds it. `--help` advertises it as "Self-update", but
the implementation prints instructions to re-clone and `cargo install`. The
command lies about what it does.

jcode ships `curl -fsSL https://jcode.sh/install | bash`, a PowerShell
installer, and a Homebrew tap, plus `release.yml` and `uninstall` scripts.

## Goal

A user on Linux, macOS or Windows can install a working whycode without a Rust
toolchain, and `whycode upgrade` actually upgrades.

## Scope

In:

- Tagged releases producing binaries for the three platforms already built in
  CI, plus checksums.
- `install.sh` and `install.ps1` that download the right artifact, verify its
  checksum and place it on `PATH`.
- `uninstall.sh` / `uninstall.ps1`.
- A real `whycode upgrade`: check the latest release, compare versions,
  download, verify, replace the running binary.
- Version output that includes the commit and build date.

Out (full automation — partial scaffolding exists):

- Dedicated Homebrew tap repo, AUR, winget, Nix. Each is ongoing maintenance.
  **Done (2026-08-14):** in-repo binary `Formula/whycode.rb` for `v0.1.0`;
  `release.yml` re-runs `scripts/update_homebrew_formula.sh` after each tag.
  See [packaging.md](../packaging.md). Dedicated tap repo still later.
- Code signing and notarization. macOS Gatekeeper will warn. Document it
  rather than pretending otherwise.
- A hosted install domain. Use the raw GitHub URL until the rest works.

## Tasks

- [x] `release.yml`: on tag `v*`, build release binaries for
      `ubuntu-latest`, `macos-latest`, `windows-latest`
- [x] Build both macOS architectures (`aarch64-apple-darwin`,
      `x86_64-apple-darwin`) — Apple silicon is the common case now
- [x] Emit `SHA256SUMS` and attach every artifact to the GitHub release
- [x] `scripts/install.sh`: detect OS and architecture, download, verify
      checksum, install to `~/.local/bin`, warn if it is not on `PATH`
- [x] `scripts/install.ps1`: the same for Windows, installing under
      `%LOCALAPPDATA%\Programs\whycode`
- [x] `scripts/uninstall.sh` and `scripts/uninstall.ps1`
- [x] Implement `cmd_upgrade` against the GitHub releases API, with atomic
      replacement (download to a temp path, verify, rename over the target)
- [x] `--version` reports version, short commit hash and build date
      (`crates/cli/build.rs` → `whycode 0.1.0 (<hash> <YYYY-MM-DD>)`)
- [x] README: install section leads with the scripts, source build second
- [x] Partial Homebrew: `Formula/whycode.rb` (HEAD/source) +
      `scripts/update_homebrew_formula.sh` for post-release binary formula
- [x] Homebrew: binary formula for `v0.1.0` + `release.yml` auto-bump on
      later tags
- [ ] winget / AUR / Nix (on demand)

## Acceptance criteria

- [x] Tagging `v0.1.0` produces a release with binaries for all three
      platforms plus `SHA256SUMS`, with no manual step
      (2026-08-04: [release v0.1.0](https://github.com/whycorporation/whycode/releases/tag/v0.1.0))
- [x] `install.sh` on a machine with no Rust toolchain yields a runnable
      `whycode --version` — verified with `GITHUB_TOKEN` while the repo is
      **private** (anonymous `/releases/download/` URLs 404 until the repo is public)
- [ ] `install.ps1` does the same on Windows (not exercised here; same asset set)
- [x] A tampered artifact fails checksum verification (local digest mismatch check)
- [x] `whycode upgrade` on the newest version says so and exits 0 (with token on private)
- [x] `whycode upgrade` failing mid-download leaves the existing binary intact
      (unit: `replace_binary` restore path)
- [x] Uninstall removes the binary and reports what it removed; it does not
      touch the config or data directories without `--purge`
      (`scripts/uninstall.sh` / `scripts/uninstall.ps1`)

**Ship gate remaining:** make `whycorporation/whycode` **public** so
`curl … | sh` works without a token. Private installs work today via
`GITHUB_TOKEN` / `GH_TOKEN` (API asset download).

## Risks

- **Replacing a running binary.** Windows locks the executable in use. The
  usual approach is to rename the running file and move the new one into
  place; verify on Windows specifically.
- **`PATH` on Windows.** Modifying user `PATH` from an installer is intrusive
  and easy to get wrong. Print the directory and let the user opt in.
- **Unsigned binaries.** Expect macOS and SmartScreen warnings. Document the
  exact wording users will see.

## Reference

`jcode/scripts/install.sh`, `install.ps1`, `uninstall.sh`, `uninstall.ps1`,
`install_release.sh`, and `.github/workflows/release.yml`. Also
`test_install_conversion.sh` and `test_windows_launcher_install.ps1` — they
test the installers, which is the part usually left untested.

## Verification status

Everything in Tasks is written and its syntax is checked in CI. The acceptance
criteria left unticked all require a published release to exercise, and cutting
one is an outward-facing action with a version number attached — a deliberate
maintainer decision, not a side effect of implementing the workflow.

To exercise them:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

That runs `.github/workflows/release.yml`, which builds all four targets,
generates `SHA256SUMS` and publishes the release. The installers and
`whycode upgrade` can then be run against it, and the remaining boxes ticked.
After a green release, `release.yml` runs `scripts/update_homebrew_formula.sh`
and commits `Formula/whycode.rb` to `main`. Manual backfill is the same script.

Unit-tested without a release: version comparison, `SHA256SUMS` parsing
(including the `*` binary marker `sha256sum` writes), and that `replace_binary`
leaves no staging files behind and restores the original if the rename fails.