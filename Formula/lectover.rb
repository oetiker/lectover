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
  version "0.0.0"
  license "AGPL-3.0-or-later"

  # BOTTLE-START
  # BOTTLE-END

  on_macos do
    on_arm do
      url "https://github.com/oetiker/lectover/releases/download/v#{version}/lectover-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # mac-arm
    end
  end

  def install
    bin.install "lectover"
    Dir["man/*.[1-9]"].each { |page| (man/"man#{page[-1]}").install page }
    pkgshare.install "lectover.js"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/lectover --version")
  end
end
