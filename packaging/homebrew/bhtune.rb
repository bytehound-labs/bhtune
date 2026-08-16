# frozen_string_literal: true

# Prepared ahead of standing up the `bytehound-labs/homebrew-bhtune` tap repo (deliberately
# not yet created -- see AGENTS.md's "Packaging and distribution" section, `pkg-eval-homebrew`
# under `pkg-evaluate-others`). This file is not installable as-is: it belongs at
# `Formula/bhtune.rb` in that tap once it exists, and every `sha256` placeholder below must
# be filled in from a real release's checksums.txt first -- there is no release yet to
# compute them from.
#
# Supports only the two platforms `release.yml`'s build matrix actually produces: Linux
# x86_64 and macOS arm64 (Apple Silicon). There is no Intel Mac or Linux ARM archive to
# point at -- matches the "one package, not two"/opcda-bridge-derived precedent documented
# in AGENTS.md. `brew install` on any other platform fails naturally with "no url" for the
# current platform rather than silently installing something that doesn't exist.
#
# Installs only the two binaries plus LICENSE/README, matching exactly what's in the
# release archive today. Man pages and shell completions are deliberately left out rather
# than widening `release.yml`'s `upload-rust-binary-action` `include:` list to add them:
# that input has no glob support (confirmed in its own `action.yml`) and would need every
# one of the ~19 auto-generated man pages named individually, which would drift out of
# sync with `docs-generated-cli`'s whole point -- that generated docs can never silently
# drift. Revisit if that becomes worth the maintenance cost.
class Bhtune < Formula
  desc "MRFT PID auto-tuner (CLI + HTTP API/web GUI) for industrial control loops, driven over OPC DA via the separate opcda-bridge gateway"
  homepage "https://github.com/bytehound-labs/bhtune"
  license "AGPL-3.0-or-later"
  version "0.1.0"

  on_macos do
    on_arm do
      url "https://github.com/bytehound-labs/bhtune/releases/download/v#{version}/bhtune-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_REAL_SHA256_FROM_RELEASE_CHECKSUMS_TXT"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/bytehound-labs/bhtune/releases/download/v#{version}/bhtune-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_REAL_SHA256_FROM_RELEASE_CHECKSUMS_TXT"
    end
  end

  def install
    bin.install "bhtune"
    bin.install "bhtune-server"
    doc.install "README.md"
    # Not legally required for AGPL redistribution (this formula's `license` field above
    # already declares it to `brew audit`), but keeps the full license text alongside the
    # binaries either way, matching the `.deb`/`.rpm` packages' own
    # `/usr/share/doc/bhtune/LICENSE` convention.
    doc.install "LICENSE"
  end

  # `bhtune-server` is the long-running HTTP API/web GUI process; `bhtune` (the headless
  # CLI) is a one-shot command with nothing to keep alive, so it has no `service` block.
  service do
    run [opt_bin/"bhtune-server"]
    keep_alive false
    log_path var/"log/bhtune-server.log"
    error_log_path var/"log/bhtune-server.log"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/bhtune --version")
  end
end
