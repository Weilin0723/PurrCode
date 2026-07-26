class Purrcode < Formula
  desc "PurrCode local-first coding agent with the PawGate judgment runtime"
  homepage "https://github.com/Weilin0723/PurrCode"
  version "0.1.0"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/Weilin0723/PurrCode/releases/download/v#{version}/purrcode-aarch64-apple-darwin.tar.gz"
      sha256 "RELEASE_AUTOMATION_REPLACES_THIS_VALUE"
    else
      url "https://github.com/Weilin0723/PurrCode/releases/download/v#{version}/purrcode-x86_64-apple-darwin.tar.gz"
      sha256 "RELEASE_AUTOMATION_REPLACES_THIS_VALUE"
    end
  end

  def install
    bin.install "purrcode"
    bin.install "purrcoded"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/purrcode --version")
  end
end
