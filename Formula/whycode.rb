# typed: false
# frozen_string_literal: true

# Homebrew formula for whycode.
#
# Partial packaging (phase 2 follow-up):
#   - Source / HEAD install works today (needs a Rust toolchain via brew).
#   - Prebuilt binary bottles per platform will be filled in by
#     `scripts/update_homebrew_formula.sh` after a tagged GitHub release.
#
# Install (from this repo, no separate tap required yet):
#   brew tap whycorporation/whycode https://github.com/whycorporation/whycode
#   brew install --HEAD whycode
#
# Or without tapping:
#   brew install --HEAD --formula \
#     https://raw.githubusercontent.com/whycorporation/whycode/main/Formula/whycode.rb

class Whycode < Formula
  desc "Terminal coding agent written in Rust"
  homepage "https://github.com/whycorporation/whycode"
  license "MIT"
  head "https://github.com/whycorporation/whycode.git", branch: "main"

  # Stable source tarball — enabled when the first release is cut.
  # `scripts/update_homebrew_formula.sh` rewrites url/sha256/version.
  # url "https://github.com/whycorporation/whycode/archive/refs/tags/v0.1.0.tar.gz"
  # sha256 "REPLACE_ME"
  # version "0.1.0"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/cli")
  end

  test do
    assert_match "whycode", shell_output("#{bin}/whycode --version")
  end
end
