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
