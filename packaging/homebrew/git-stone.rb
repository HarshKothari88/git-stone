# Template for the formula in the tap repository HarshKothari88/homebrew-tap (Formula/git-stone.rb).
# After each release, set `version` and the four sha256 values from the files `git-stone-<target>.tar.gz.sha256`.
class GitStone < Formula
  desc "Save task boundaries as you work, turn them into atomic Git commits later"
  homepage "https://github.com/HarshKothari88/git-stone"
  version "0.1.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/HarshKothari88/git-stone/releases/download/v#{version}/git-stone-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE"
    end
    on_intel do
      url "https://github.com/HarshKothari88/git-stone/releases/download/v#{version}/git-stone-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE"
    end
  end
  on_linux do
    on_arm do
      url "https://github.com/HarshKothari88/git-stone/releases/download/v#{version}/git-stone-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE"
    end
    on_intel do
      url "https://github.com/HarshKothari88/git-stone/releases/download/v#{version}/git-stone-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE"
    end
  end

  depends_on "git"

  def install
    bin.install "git-stone"
    man1.install "git-stone.1"
  end

  test do
    assert_match "git-stone #{version}", shell_output("#{bin}/git-stone version")
  end
end
