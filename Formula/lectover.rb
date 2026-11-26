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
  version "0.4.0"
  license "AGPL-3.0-or-later"

  # BOTTLE-START
  bottle do
    root_url "https://github.com/oetiker/lectover/releases/download/v0.4.0"
    sha256 cellar: :any_skip_relocation, arm64_big_sur: "2ded4819f843b0d2403d6fa0892e49833fc99fe1e0ea5b3c49eec073b6d4e515"
  end
  # BOTTLE-END

  url "https://github.com/oetiker/lectover/releases/download/v#{version}/lectover-#{version}-aarch64-apple-darwin.tar.gz"
  sha256 "30449be7250723b16181bb27cb1ff74ba3afd67fbbdf461402bd54e55afc9a70" # mac-arm

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
