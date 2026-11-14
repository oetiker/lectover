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
  version "0.1.0"
  license "AGPL-3.0-or-later"

  # BOTTLE-START
  bottle do
    root_url "https://github.com/oetiker/lectover/releases/download/v0.1.0"
    sha256 cellar: :any_skip_relocation, arm64_big_sur: "eb07e7ae93a0086e4996381dadc743f8e1af4fd7ca4db7b947c6ed8affc6a3d0"
  end
  # BOTTLE-END

  on_macos do
    on_arm do
      url "https://github.com/oetiker/lectover/releases/download/v#{version}/lectover-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "7ab6f66a2a49c4bfb63fa608c310027fd563267db772cd5bd92776fc6b5287a0" # mac-arm
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
