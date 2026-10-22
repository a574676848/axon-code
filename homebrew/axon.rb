# encoding: UTF-8

Encoding.default_external = Encoding::UTF_8
Encoding.default_internal = Encoding::UTF_8

# Axon Homebrew Formula
# 托管在自建 tap：https://github.com/a574676848/homebrew-tap
#
# 用户使用方式：
#   brew tap a574676848/tap
#   brew install axon
#   brew upgrade axon
#
# CI 自动更新：release.yml 发布后更新 sha256 值
class Axon < Formula
  desc "Model-routed agentic coding TUI with provider load balancing, memory, and accurate tool calls"
  homepage "https://github.com/a574676848/axon-code"
  version "__VERSION__"
  license "MIT"

  # ── macOS Apple Silicon ────────────────────────────────────────────────────
  on_macos do
    on_arm do
      url "https://github.com/a574676848/axon-code/releases/download/v#{version}/axon-v#{version}-aarch64-apple-darwin.tar.gz"
      # 在 CI 中由 release.yml 自动更新此 sha256
      sha256 "__MACOS_ARM64_SHA256__"
    end

    # ── macOS Intel ──────────────────────────────────────────────────────────
    on_intel do
      url "https://github.com/a574676848/axon-code/releases/download/v#{version}/axon-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "__MACOS_X64_SHA256__"
    end
  end

  # ── Linux x86_64 ──────────────────────────────────────────────────────────
  on_linux do
    on_intel do
      url "https://github.com/a574676848/axon-code/releases/download/v#{version}/axon-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "__LINUX_X64_SHA256__"
    end

    on_arm do
      url "https://github.com/a574676848/axon-code/releases/download/v#{version}/axon-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "__LINUX_ARM64_SHA256__"
    end
  end

  def install
    bin.install "axon"
    pkgshare.install "assets/defaults/SOUL.md"
  end

  def post_install
    axon_dir = Pathname.new(Dir.home)/".axon"
    soul_path = axon_dir/"SOUL.md"
    return if soul_path.exist? || soul_path.symlink?

    axon_dir.mkpath
    cp pkgshare/"SOUL.md", soul_path
    soul_path.chmod 0600
  end

  def caveats
    <<~EOS
      Axon 是一个模型路由与记忆驱动的 Agentic 编码工作台，提供多供应商负载、Memory 自进化、高缓存复用和高准确率工具调用。

      首次运行：
        axon --help

      更新 Axon：
        brew upgrade axon

      更多信息：https://github.com/a574676848/axon-code
    EOS
  end

  test do
    # 验证二进制可执行且版本号匹配
    output = shell_output("#{bin}/axon --version")
    assert_match version.to_s, output
  end
end
