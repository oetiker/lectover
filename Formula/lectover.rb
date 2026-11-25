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
  version "0.3.2"
  license "AGPL-3.0-or-later"

  # BOTTLE-START
  bottle do
    root_url "https://github.com/oetiker/lectover/releases/download/v0.3.2"
    sha256 cellar: :any_skip_relocation, arm64_big_sur: "b5bbd3e46796b3e6f790b2dc87ec2a004ee1ee424228d1d1147359105fbe636f"
  end
  # BOTTLE-END

  url "https://github.com/oetiker/lectover/releases/download/v#{version}/lectover-#{version}-aarch64-apple-darwin.tar.gz"
  sha256 "eea5564f4b9c1cbd6b367d1e5de9f42554b54c338fcecc2105065210ef2ba55b" # mac-arm

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
