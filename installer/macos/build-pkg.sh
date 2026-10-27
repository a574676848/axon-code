#!/usr/bin/env bash
# Axon macOS .pkg 构建脚本
# 用法：bash build-pkg.sh <VERSION> <BINARY_PATH> <LAUNCHER_PATH> <PACKAGE_NAME>
# 示例：bash build-pkg.sh 0.2.0 target/aarch64-apple-darwin/release/axon axon-v0.2.0-aarch64-apple-darwin
#
# 前置条件：macOS 系统，已安装 Xcode Command Line Tools
# 产物：dist/<NAME>.pkg

set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
VERSION="${1:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "${ROOT_DIR}/Cargo.toml" | head -1)}"
[ -n "${VERSION}" ] || { echo "无法从 Cargo.toml 读取工作区版本号" >&2; exit 1; }
BINARY="${2:?缺少二进制路径}"
LAUNCHER="${3:?缺少 launcher 路径}"
VOICE_HOST="${5:-}"
NAME="${4:?缺少包名}"

WORK_DIR="$(mktemp -d)"
DIST_DIR="${ROOT_DIR}/dist"

mkdir -p "${DIST_DIR}"

# 验证二进制存在
if [ ! -f "${BINARY}" ] || [ ! -f "${LAUNCHER}" ]; then
    echo "错误：Axon 二进制或 launcher 不存在" >&2
    exit 1
fi

echo "=== 构建 Axon macOS .pkg ==="
echo "版本：${VERSION}"
echo "二进制：${BINARY}"
echo "包名：${NAME}"

# ── 构建 payload 目录结构 ────────────────────
PAYLOAD="${WORK_DIR}/payload"
INSTALL_ROOT="${PAYLOAD}/usr/local/share/axon"
DEFAULTS_DIR="${INSTALL_ROOT}/defaults"
RELEASE_NAME="${VERSION}"
mkdir -p "${INSTALL_ROOT}/versions/${RELEASE_NAME}" "${INSTALL_ROOT}/launcher" "${DEFAULTS_DIR}"
cp "${BINARY}" "${INSTALL_ROOT}/versions/${RELEASE_NAME}/axon"
cp "${LAUNCHER}" "${INSTALL_ROOT}/launcher/axon-launcher"
if [ -n "${VOICE_HOST}" ]; then
    if [ ! -x "${VOICE_HOST}" ]; then echo "缺少语音宿主：${VOICE_HOST}" >&2; exit 1; fi
    cp "${VOICE_HOST}" "${INSTALL_ROOT}/versions/${RELEASE_NAME}/axon-voice-host"
fi
cp "${ROOT_DIR}/assets/defaults/SOUL.md" "${DEFAULTS_DIR}/SOUL.md"
chmod 755 "${INSTALL_ROOT}/versions/${RELEASE_NAME}/axon" "${INSTALL_ROOT}/launcher/axon-launcher"
if [ -n "${VOICE_HOST}" ]; then chmod 755 "${INSTALL_ROOT}/versions/${RELEASE_NAME}/axon-voice-host"; fi
chmod 644 "${DEFAULTS_DIR}/SOUL.md"

# ── 构建组件 pkg ────────────────────────────
COMPONENT_PKG="${WORK_DIR}/axon-component.pkg"
SCRIPTS_DIR="${WORK_DIR}/scripts"
cp -R "${SCRIPT_DIR}/scripts" "${SCRIPTS_DIR}"
sed -i '' "s/__VERSION__/${VERSION}/g" "${SCRIPTS_DIR}/postinstall"
sed -i '' "s/__RELEASE_NAME__/${RELEASE_NAME}/g" "${SCRIPTS_DIR}/postinstall"
pkgbuild \
    --root "${PAYLOAD}" \
    --identifier "com.github.a574676848.axon" \
    --version "${VERSION}" \
    --install-location "/" \
    --scripts "${SCRIPTS_DIR}" \
    "${COMPONENT_PKG}"

echo "组件 pkg 已生成：${COMPONENT_PKG}"

# ── 合成 Distribution .pkg ──────────────────
DIST_XML="${WORK_DIR}/Distribution.xml"
sed "s/__VERSION__/${VERSION}/g" "${SCRIPT_DIR}/Distribution.xml" > "${DIST_XML}"

OUTPUT="${DIST_DIR}/${NAME}.pkg"
productbuild \
    --distribution "${DIST_XML}" \
    --package-path "${WORK_DIR}" \
    --version "${VERSION}" \
    "${OUTPUT}"

echo "=== .pkg 构建完成 ==="
echo "产物：${OUTPUT}"
echo "大小：$(du -sh "${OUTPUT}" | awk '{print $1}')"

# 清理解压目录
rm -rf "${WORK_DIR}"
