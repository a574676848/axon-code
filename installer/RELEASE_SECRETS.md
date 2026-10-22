# Axon 发布流水线 — Secrets 配置指南

## 必需 Secrets（无签名也能发布）

| Secret               | 说明                                 |
|----------------------|--------------------------------------|
|                      | 无需配置任何 secret 即可发布 zip/tar.gz/msi/pkg/deb |

## 可选 Secrets（签名增强）

### Windows Authenticode 签名

在 GitHub 仓库 Settings → Secrets → Actions 中添加：

| Secret                | 说明                                                     |
|-----------------------|----------------------------------------------------------|
| `WINDOWS_CERT_BASE64` | EV/OV 代码签名证书 .pfx 的 Base64 编码                    |
| `WINDOWS_CERT_PASSWORD` | .pfx 证书密码                                          |

### macOS Developer ID 签名 + 公证

需要 Apple Developer Program 成员资格（$99/年）。

| Secret                   | 说明                                     |
|--------------------------|------------------------------------------|
| `MACOS_CERT_BASE64`      | Developer ID Application .p12 证书 Base64 |
| `MACOS_CERT_PASSWORD`    | .p12 证书密码                             |
| `MACOS_TEAM_ID`          | Apple Developer Team ID （在 developer.apple.com 查看） |
| `MACOS_NOTARY_USER`      | Apple ID 邮箱（用于公证）                 |
| `MACOS_NOTARY_PASSWORD`  | App-Specific Password（在 appleid.apple.com 生成） |

### Linux GPG 签名

| Secret            | 说明                             |
|-------------------|----------------------------------|
| `GPG_PRIVATE_KEY` | GPG 私钥（`gpg --export-secret-key` 导出） |
| `GPG_PASSPHRASE`  | GPG 密钥密码                     |

## 签名触发机制

所有签名步骤均设置了 `continue-on-error: true`，**签名失败不会阻断发布**。
相关条件：
- Windows 签名：仅当 `secrets.WINDOWS_CERT_BASE64` 存在时执行
- macOS 签名：仅当 `secrets.MACOS_CERT_BASE64` 存在时执行
- macOS 公证：仅当 `secrets.MACOS_NOTARY_PASSWORD` 存在时执行
- Linux GPG：仅当 `secrets.GPG_PRIVATE_KEY` 存在时执行

## 首次发布流程

```bash
# 1. 在 GitHub 仓库 Settings → Secrets 中配置（可暂为空）

# 2. 打 tag
git tag v0.2.0
git push origin v0.2.0

# 3. 等待 GitHub Actions 完成 → Releases 页面即出现各类 Assets
```