#!/usr/bin/env bash
# Axon macOS .pkg 公证脚本
# 在 GitHub Actions 的 macos_pkg job 中调用（codesign 后）
#
# 前置条件：
#   - secrets.MACOS_CERT_BASE64：Developer ID Application 证书
#   - secrets.MACOS_TEAM_ID：Apple Developer Team ID
#   - secrets.MACOS_NOTARY_USER：Apple ID（app-specific password）
#   - secrets.MACOS_NOTARY_PASSWORD：App-Specific Password
#
# 参考：https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution

set -euo pipefail

PKG_PATH="${1:?缺少 .pkg 路径}"
TEAM_ID="${MACOS_TEAM_ID:?缺少 MACOS_TEAM_ID}"
NOTARY_USER="${MACOS_NOTARY_USER:?缺少 MACOS_NOTARY_USER}"
NOTARY_PASSWORD="${MACOS_NOTARY_PASSWORD:?缺少 MACOS_NOTARY_PASSWORD}"

echo "=== 提交 macOS 公证 ==="
echo "包路径：${PKG_PATH}"

# 提交公证
xcrun notarytool submit "${PKG_PATH}" \
    --team-id "${TEAM_ID}" \
    --apple-id "${NOTARY_USER}" \
    --password "${NOTARY_PASSWORD}" \
    --wait

# 钉装公证票据
xcrun stapler staple "${PKG_PATH}"

echo "=== 公证完成：${PKG_PATH} ==="