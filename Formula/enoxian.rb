# Homebrew formula for enoxian (enox + enoxd).
#
# The version and per-asset SHA256 values are updated by the optional Homebrew
# tap job in .github/workflows/release.yml after a release is published.
class Enoxian < Formula
  desc "Peer-to-peer collaboration layer for humans and AI agents"
  homepage "https://github.com/suzent/enoxian"
  version "0.2.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/suzent/enoxian/releases/download/v#{version}/enoxian-macos-aarch64.tar.gz"
      sha256 "b9a1d7a63532c4d4e3c9ea18374097a596799eae625c076653891de2f945dc8a"
    end
    on_intel do
      url "https://github.com/suzent/enoxian/releases/download/v#{version}/enoxian-macos-x86_64.tar.gz"
      sha256 "512d35fe222827fc31e6e2d7aa6f2e332c7b37d1ac0bf84dce802ffdaa6a3724"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/suzent/enoxian/releases/download/v#{version}/enoxian-linux-aarch64.tar.gz"
      sha256 "27ca82c3a7cff87c3d8f93d5439cd64803c5f7a4c76a0000282ab1aecf7d81"
    end
    on_intel do
      url "https://github.com/suzent/enoxian/releases/download/v#{version}/enoxian-linux-x86_64.tar.gz"
      sha256 "f059e4d9c58c6c425623b269ee89b8300918c5f49fab19fb97935c405ff32501"
    end
  end

  def install
    bin.install "enox"
    bin.install "enoxd"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/enox --version")
    assert_match version.to_s, shell_output("#{bin}/enoxd --version")
  end
end
