# Homebrew formula for enoxian (enox + enoxd).
#
# This installs the prebuilt release binaries. It is a *scaffold*: the `version`,
# `url`s, and `sha256`s below must be updated for each release — the SHAs cannot
# exist until the release artifacts are published. A `scripts/bump.sh`-style step
# should regenerate them at release time (compute with `shasum -a 256 <asset>`).
#
# Usage once published in a tap (e.g. `suzent/tap`):
#   brew install suzent/tap/enoxian
class Enoxian < Formula
  desc "Peer-to-peer collaboration layer for humans and AI agents"
  homepage "https://github.com/suzent/enoxian"
  version "0.1.4"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/suzent/enoxian/releases/download/v#{version}/enoxian-macos-aarch64.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # TODO: fill at release
    end
    on_intel do
      url "https://github.com/suzent/enoxian/releases/download/v#{version}/enoxian-macos-x86_64.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # TODO: fill at release
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/suzent/enoxian/releases/download/v#{version}/enoxian-linux-aarch64.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # TODO: fill at release
    end
    on_intel do
      url "https://github.com/suzent/enoxian/releases/download/v#{version}/enoxian-linux-x86_64.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # TODO: fill at release
    end
  end

  def install
    bin.install "enox"
    bin.install "enoxd"
  end

  test do
    assert_match "enox", shell_output("#{bin}/enox --help")
  end
end
