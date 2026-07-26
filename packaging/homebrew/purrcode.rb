class Purrcode < Formula
  desc "PurrCode local-first coding agent with the PawGate judgment runtime"
  homepage "https://github.com/Weilin0723/PurrCode"
  version "0.2.1"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/Weilin0723/PurrCode/releases/download/v#{version}/purrcode-aarch64-apple-darwin.tar.gz"
      sha256 "28d6627b2deb9d5d140323f578ad16e66051fb9880acc6cf918cf3ac08854361"
    else
      url "https://github.com/Weilin0723/PurrCode/releases/download/v#{version}/purrcode-x86_64-apple-darwin.tar.gz"
      sha256 "20c8b17bf4d4326a037ccb365286e33c4bd2a0771c249e377752b5d044c5c363"
    end
  end

  def install
    target = Hardware::CPU.arm? ? "aarch64-apple-darwin" : "x86_64-apple-darwin"
    bin.install "purrcode-#{target}/purrcode"
    bin.install "purrcode-#{target}/purrcoded"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/purrcode --version")
  end
end
