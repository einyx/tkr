class Tkr < Formula
  desc "Token-optimized CLI proxy for LLM development workflows"
  homepage "https://github.com/einyx/tkr"
  license "Apache-2.0"
  version "0.1.0"

  on_macos do
    on_arm do
      url "https://github.com/einyx/tkr/releases/download/v#{version}/tkr-aarch64-apple-darwin.tar.gz"
      sha256 "b3664c9a1d1708c6b0295a5e499431eb531375165d46a28b9cb88d02856ea7ed"
    end
    on_intel do
      url "https://github.com/einyx/tkr/releases/download/v#{version}/tkr-x86_64-apple-darwin.tar.gz"
      sha256 "4684fc64c6fdb41848c569a62d641de9765f7883184d4f97b2c3b604183081ca"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/einyx/tkr/releases/download/v#{version}/tkr-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "a9abe544488b5ad2f83d80e518e110e87f9dcdfab09cda2e0c8e2c9deeeb5ceb"
    end
    on_intel do
      url "https://github.com/einyx/tkr/releases/download/v#{version}/tkr-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0ce63d862a727519c9b66f56c84a3db5f2726dbd8820d5a6284e49d7e5105173"
    end
  end

  def install
    bin.install "tkr"
  end

  test do
    assert_match "tkr", shell_output("#{bin}/tkr --help")
  end
end
