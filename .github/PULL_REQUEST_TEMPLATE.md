## 变更说明

<!-- 说明为什么改、改了什么，以及关键设计取舍 -->

## 关联 Issue

<!-- 例如 Fixes #123 -->

## 变更类型

- [ ] 缺陷修复
- [ ] 新功能
- [ ] 重构（不改变外部行为）
- [ ] 文档
- [ ] 构建、依赖或 CI

## 影响面

<!-- 涉及的 crate、命令、配置键、网关路由或界面；破坏性变更必须说明迁移方式 -->

## 验证证据

<!-- 粘贴真实回执：测试输出、门禁结果、关键截图或 CDP 的 OK 行 -->

- [ ] `cargo fmt --all --check`
- [ ] `cargo check --workspace`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `pwsh -File tools/verify-architecture.ps1`
- [ ] `pwsh -File tools/verify-docs.ps1`
- [ ] 触及 `crates/axon-desktop/ui` 时已执行 `npm ci`、`npm run build`、`npm run lint`、`npm test` 与 `pwsh -File tools/verify-ui-contract.ps1`

## 文档与 Skill 同步

- [ ] 已判定 `docs/` 与 `docs/skills/operate-axon/` 是否需要同步，并在此说明依据

## 提交前自检

- [ ] 未包含本机绝对路径、凭据、私钥与运行态产物
- [ ] 提交信息为中文，且不含 en/em 破折号
