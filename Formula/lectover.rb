# Homebrew formula for lectover. This repository is its own tap.
#
# The release build rewrites the version line, the mac-arm sha256 line by its
# marker comment and the bottle block between the BOTTLE markers. Keep the
# markers, and keep the url line directly above its sha256 line.
#
# Apple silicon only: the release builds macOS for aarch64 with Metal, and no
# Intel or Linux archive the formula could name (see native-targets in
# .github/repo-infra.json).
class Lectover < Formula
  desc "Read-aloud audio with word timings for Ghost blog posts"
  homepage "https://github.com/oetiker/lectover"
  version "0.2.1"
  license "AGPL-3.0-or-later"

  # BOTTLE-START
  bottle do
    root_url "https://github.com/oetiker/lectover/releases/download/v0.2.1"
    sha256 cellar: :any_skip_relocation, arm64_big_sur: "1fee0d5939af431af3b83ea5d498209dcf92910ef4358e91693cd96c5efc021f"
  end
  # BOTTLE-END

  url "https://github.com/oetiker/lectover/releases/download/v#{version}/lectover-#{version}-aarch64-apple-darwin.tar.gz"
  sha256 "4fcd56fa462daac4d0242f4a284f06b2da10563c5b859b3bdbf7a761d56f8d1b" # mac-arm

  # A url inside on_macos/on_arm leaves Linux and Intel Macs with no url at
  # all, and brew then fails with "formula requires at least a URL" and a
  # request to report a Homebrew bug. These two make it refuse cleanly.
  depends_on arch: :arm64
  depends_on :macos

  def install
    bin.install "lectover"
    Dir["man/*.[1-9]"].each { |page| (man/"man#{page[-1]}").install page }
    pkgshare.install "lectover.js"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/lectover --version")
  end
end
