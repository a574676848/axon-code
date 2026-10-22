#!/usr/bin/env bash
# Axon .deb 构建脚本
# 在 GitHub Actions 的 linux_deb job 中调用
#
# 环境变量：
#   AXON_VERSION  — 版本号（如 0.2.0）
#   AXON_ARCH     — Debian 架构名（amd64 / arm64）
#   AXON_BINARY   — 编译好的 Axon 二进制路径
#   AXON_LAUNCHER — 编译好的稳定 launcher 路径
#
# 产物：dist/axon-v<VERSION>-<target>.deb

set -euo pipefail

ROOT_HINT="$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)"
VERSION="${AXON_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "${ROOT_HINT}/Cargo.toml" | head -1)}"
[ -n "${VERSION}" ] || { echo "无法从 Cargo.toml 读取工作区版本号" >&2; exit 1; }
ARCH="${AXON_ARCH:?缺少 AXON_ARCH}"
BINARY="${AXON_BINARY:?缺少 AXON_BINARY}"
LAUNCHER="${AXON_LAUNCHER:?缺少 AXON_LAUNCHER}"

# 确定 Rust target triple
case "${ARCH}" in
    amd64) TARGET="x86_64-unknown-linux-gnu" ;;
    arm64) TARGET="aarch64-unknown-linux-gnu" ;;
    *)     echo "不支持的架构：${ARCH}" >&2; exit 1 ;;
esac

NAME="axon-v${VERSION}-${TARGET}"

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
WORK_DIR="$(mktemp -d)"
DIST_DIR="${ROOT_DIR}/dist"

mkdir -p "${DIST_DIR}"

echo "=== 构建 Axon .deb 包 ==="
echo "版本：${VERSION}"
echo "架构：${ARCH} (${TARGET})"
echo "二进制：${BINARY}"

# ── 复制模板目录 ──────────────────────────────
DEB_SRC="${SCRIPT_DIR}/deb"
DEB_DIR="${WORK_DIR}/${NAME}"

cp -a "${DEB_SRC}" "${DEB_DIR}"

# ── 复制二进制 ────────────────────────────────
if [ ! -f "${BINARY}" ] || [ ! -f "${LAUNCHER}" ]; then
    echo "错误：Axon 二进制或 launcher 不存在" >&2
    exit 1
fi
INSTALL_ROOT="${DEB_DIR}/usr/share/axon"
DEFAULTS_DIR="${INSTALL_ROOT}/defaults"
RELEASE_NAME="${VERSION}"
mkdir -p "${INSTALL_ROOT}/versions/${RELEASE_NAME}" "${INSTALL_ROOT}/launcher" "${DEFAULTS_DIR}"
cp "${BINARY}" "${INSTALL_ROOT}/versions/${RELEASE_NAME}/axon"
cp "${LAUNCHER}" "${INSTALL_ROOT}/launcher/axon-launcher"
cp "${ROOT_DIR}/assets/defaults/SOUL.md" "${DEFAULTS_DIR}/SOUL.md"
chmod 755 "${INSTALL_ROOT}/versions/${RELEASE_NAME}/axon" "${INSTALL_ROOT}/launcher/axon-launcher"
chmod 644 "${DEFAULTS_DIR}/SOUL.md"

# ── 替换 control 文件中的占位符 ───────────────
sed -i "s/__VERSION__/${VERSION}/g" "${DEB_DIR}/DEBIAN/control"
sed -i "s/__ARCH__/${ARCH}/g" "${DEB_DIR}/DEBIAN/control"
sed -i "s/__VERSION__/${VERSION}/g" "${DEB_DIR}/DEBIAN/postinst"
sed -i "s/__RELEASE_NAME__/${RELEASE_NAME}/g" "${DEB_DIR}/DEBIAN/postinst"

# ── 修复脚本权限 ──────────────────────────────
chmod 755 "${DEB_DIR}/DEBIAN/postinst"
chmod 755 "${DEB_DIR}/DEBIAN/prerm"
chmod 755 "${DEB_DIR}/DEBIAN/preinst"

# ── 构建 .deb ─────────────────────────────────
OUTPUT="${DIST_DIR}/${NAME}.deb"
dpkg-deb --build "${DEB_DIR}" "${OUTPUT}"

# ── 清理解压目录 ──────────────────────────────
rm -rf "${WORK_DIR}"

echo "=== .deb 构建完成 ==="
echo "产物：${OUTPUT}"
echo "大小：$(du -sh "${OUTPUT}" | awk '{print $1}')"
echo "验证：dpkg -I ${OUTPUT}"
