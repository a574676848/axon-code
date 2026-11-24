#!/usr/bin/env bash
# Axon APT 仓库部署脚本
# 在 GitHub Pages 上托管 APT 仓库（通过 gh-pages 分支或 apt.axon.dev）
#
# 用法：bash deploy-apt.sh <REPO_DIR> <DEB_FILES...>
# 示例：bash deploy-apt.sh /tmp/apt-repo dist/*.deb
#
# 前置条件：reprepro（可用 apt-get install reprepro 安装）

set -euo pipefail

REPO_DIR="${1:?缺少 APT 仓库目录}"
shift

DISTRO="stable"
CODENAME="main"

echo "=== 初始化 Axon APT 仓库 ==="
echo "仓库目录：${REPO_DIR}"

mkdir -p "${REPO_DIR}/conf"

# ── reprepro 配置 ─────────────────────────────
cat > "${REPO_DIR}/conf/distributions" <<EOF
Origin: Axon
Label: Axon APT Repository
Suite: ${DISTRO}
Codename: ${CODENAME}
Architectures: amd64 arm64
Components: main
Description: Axon Local-First Agentic Coding TUI — Official APT Repository
SignWith: yes
EOF

# ── 导入所有 .deb ────────────────────────────
for deb in "$@"; do
    echo "导入：${deb}"
    reprepro -b "${REPO_DIR}" includedeb "${CODENAME}" "${deb}"
done

echo ""
echo "=== APT 仓库就绪 ==="
echo "repo 根目录：${REPO_DIR}"
echo ""
echo "部署到 GitHub Pages（手动操作）："
echo "  1. 将 ${REPO_DIR} 推送到 gh-pages 分支"
echo "  2. 在仓库 Settings → Pages 中启用"
echo ""
echo "用户端添加源："
echo "  echo 'deb [trusted=yes] https://a574676848.github.io/axon-code/apt stable main' | sudo tee /etc/apt/sources.list.d/axon.list"
echo "  sudo apt update"
echo "  sudo apt install axon"
