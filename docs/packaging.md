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
brew install whycorporation/whycodes/whycodes
brew upgrade whycodes

# Tip of main (needs Rust via brew)
brew install --HEAD whycorporation/whycodes/whycodes

# Already tapped, but Homebrew 6+ refused the untrusted formula
brew trust --formula whycorporation/whycodes/whycodes
brew install whycodes

# One-shot without a persistent tap (still evaluates the formula Ruby)
brew install --formula \
  https://raw.githubusercontent.com/whycorporation/whycodes/main/Formula/whycodes.rb
```

Homebrew 6.0+ [requires tap trust](https://docs.brew.sh/Tap-Trust) for
non-official taps. Prefer the fully-qualified install (trusts only this
formula) over `brew trust whycorporation/whycodes` (trusts every current
and future formula in the tap). The custom tap URL is required because
this repo is the tap; there is no separate `homebrew-whycodes` repository.

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

## Landing (why.codes)

GitHub stars, contributor count, and the latest tag on
[why.codes](https://why.codes) come from the public API at **build time**, not
from hand-edited copy.

`release.yml` dispatches [`deploy-landing.yml`](../.github/workflows/deploy-landing.yml)
after a tagged publish. That job checks out `whycorporation/whycodes-landing`
and runs `pnpm deploy`, which refreshes `app/data/github.json` then prerenders
Cloudflare Workers.

Manual refresh (no new tag):

```bash
gh workflow run deploy-landing.yml
```

The committed JSON is a fallback so offline `pnpm build` still has numbers.

`https://why.codes/install` (and `/install.sh`, `/install.ps1`) are Nitro
routes on that Worker. Each request fetches
`scripts/install.sh` / `install.ps1` from GitHub raw (`main`) and falls back
to a snapshot baked at deploy (`server/data/install.json`). The homepage
`curl | bash` line is that alias — not a second installer.

A push to `scripts/install.sh` / `install.ps1` on `main` also runs
`deploy-landing.yml`, so the baked snapshot stays current.

### Later

- Dedicated tap repo (`whycorporation/homebrew-whycodes`) if formula noise in
  the main tree becomes annoying
- Linux aarch64 prebuild (formula currently documents intel-only binaries)
- Code signing / notarization (Gatekeeper warning stays until then)
- winget / scoop, AUR, Nix flakes

## Install scripts (already production)

See `scripts/install.sh`, `install.ps1`, and `whycodes upgrade`. Those remain
the primary path until package managers are first-class. Users should run:

```bash
curl -fsSL https://why.codes/install | bash
```

```powershell
irm https://why.codes/install.ps1 | iex
```

GitHub raw URLs still work; why.codes is the short alias.

The scripts verify the download against the release `SHA256SUMS`.
`install.sh` prints a PATH hint when `$HOME/.local/bin` is missing from
`PATH`. `install.ps1` adds `%LOCALAPPDATA%\Programs\whycodes` to the
current user's PATH (and this session) so `whycodes` works immediately
without WSL. `WHYCODES_INSTALL_DIR` overrides the Unix location;
`install.ps1 -InstallDir` overrides the Windows location. Installers and
Homebrew ship only the `whycodes` binary. Public releases need no GitHub
token; `GITHUB_TOKEN` / `GH_TOKEN` remain an optional fallback if you
point the scripts at a private fork.

`scripts/uninstall.sh` / `uninstall.ps1` remove the binary. Add `--purge` /
`-Purge` to delete config and session data as well. Homebrew users should
`brew uninstall whycodes` instead.

The binaries are unsigned, so macOS Gatekeeper and Windows SmartScreen will
warn on first run. `whycodes upgrade` replaces the running binary with the
newest release (checksum verified) and leaves the existing one in place if
anything fails — except Homebrew installs, which it will not touch.

Interactive TUI sessions also check GitHub for a newer release and offer a
home-screen confirm (`SHA256SUMS` on accept, no Homebrew prefix — those
installs get a `brew upgrade` hint instead). Disable with
`--no-auto-update`, `WHYCODES_NO_AUTO_UPDATE=1`, or
`[general] auto_update = false`. Headless / CI / ACP paths never auto-update.
This is not product telemetry: the only request is GitHub's public latest
release API.

From source, the binary lands at `target/release/whycodes`. Optional extras
(Unicode mermaid + extra highlight languages, ~+1.7 MB):

```bash
cargo build --release -p whycodes-cli --features full
```

## Install counts (no phone-home)

The CLI does not report installs or launches. The number that exists today
is how often GitHub served a release archive:

```bash
python scripts/release_downloads.py
python scripts/release_downloads.py --json
```

That is `download_count` on `.tar.gz` / `.zip` assets (`install.sh`,
Homebrew, `whycodes upgrade`, a browser click). It is **not** unique
people: upgrades, CI, and retries inflate it; `cargo install` is
invisible. `SHA256SUMS` is counted separately because the installer
fetches it next to every archive.

Do not put that total on the landing page as “users”. Homebrew-core
analytics (if the formula is accepted) is the next install graph; an
anonymous opt-out ping is only if daily-active use is still needed after
that. A WhyCodes GitHub login is not an install counter — see
[roadmap.md](roadmap.md).
