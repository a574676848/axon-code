# 开发实施准则

## 1. 核心指导原则

* **架构范式**：严格遵循 **洋葱架构 (Onion Architecture)**，确保领域层 (Domain) 的独立性与基础设施层 (Infrastructure) 的解耦。
* **设计原则**：贯彻 **SOLID** 原则，确保代码的可扩展性、健壮性与可维护性。
* **质量基准**：**拒绝** TODO 项、拒绝 Mock/Stub 实现、拒绝任何形式的简略代码。目标是直接达到生产环境的稳健性与安全性。
* **友好性原则**：全栈 **中文友好化**，确保 UI 文本、报错信息、文档说明及注释的语义清晰、地道。
* **路径无关性 (CRITICAL)**：**严禁**在源码、脚本、配置、文档与命令示例中写死任何本机相关绝对路径（用户目录、盘符、临时目录、某台机器上的安装位置）。一律用环境变量或相对路径表达：`$HOME` / `$env:USERPROFILE` / `$env:LOCALAPPDATA` / `$env:CARGO_HOME` / `%USERPROFILE%` / 仓库相对路径。安装根、版本目录与运行态根由 `AXON_INSTALL_ROOT`、`AXON_RUNTIME_ROOT` 与 launcher 解析，不得用硬编码路径替代。
* **单文件绝对严禁超过 700 行！** 如果文件超长或即将超长，**必须强制暂停堆砌代码**，并严格按照最佳开发范式（如领域驱动设计 DDD、单一职责原则 SRP、高内聚低耦合）将其拆分为多个高内聚的模块/类/组件

---

## 2. 开发工作流 (Development Workflow)

每一轮任务必须闭环，遵循以下生命周期：

### 阶段 I：实施与编码

2. **编码实现**：借鉴源码，可将优秀的源码转换成 Rust 编写完整逻辑，确保无缺失功能，移除所有临时占位符，避免重复造轮子。

#### axon-desktop 桌面端开发约束 (必须执行)

* **UI 任务前置必读 (CRITICAL)**：任何涉及 `crates/axon-desktop/ui/**` 或桌面端渲染逻辑的任务，**第一个动作**必须是完整阅读 `docs/ui-constraints.md` 与 `docs/workbench-design-system.md`，并按 `docs/ui-constraints.md` 第 8 节的开工检查表逐项确认。**未读不得修改任何界面代码**；新增颜色、字号、圆角、状态或组件形态时，必须先改这两份文档，再改实现。
* **先确认规范与 Skill，再动工**：开工前先确认是否存在相关的设计与前端开发 Skill（如 `frontend-ui-ux`），存在则加载并严格遵循其规范。仓库内的 SSOT 同样必须先读：UI 约束看 `docs/ui-constraints.md`；视觉、动效与令牌看 `docs/workbench-design-system.md`；桌面端职责边界与操作手册看 `docs/desktop.md`。禁止另起一套视觉语言、组件体系或交互范式。
* **禁止重复实现，先抽共用组件**：同一页面或不同页面之间出现**多处 80% 以上高度一致**的内容与结构时，**禁止**复制粘贴各写一遍，必须先抽象成共用组件（单一实现，差异用 props 或变体参数化）；已经是共用组件的按职责继续下沉，不得用复制绕开抽象。新写一段界面前先检索既有组件是否已覆盖该形态。
* **优先使用原生能力**：优先用 Tauri 2 与平台 WebView 的原生能力（窗口、菜单、托盘、单实例、文件对话框、剪贴板、通知、IPC 通道等），以及 Axon 既有网关协议与运行态接口取得数据；自研或引入第三方实现必须给出必要性理由。新增依赖按合同逐项登记，禁止为「看起来高级」引入自带样式的组件库、重型状态库或第二套图标体系。
* **依赖只用稳定版**：严禁 alpha、beta、rc、canary、next、insiders 等预发布线，包括把 Tauri 升到 3.x 预发布。
* **跨平台是一等公民**：Windows、macOS、Linux 三平台同时成立（`.github/workflows/ci.yml` 的 matrix 已在三平台跑门禁）。禁止写死路径分隔符、盘符、用户目录、Windows 专有 shell 或注册表假设、macOS 专有 API 假设；平台差异集中在适配层用 `cfg` 收敛或运行时能力探测，前端不得依赖仅某一平台可用的 WebView 行为。平台验证强度不一致时如实标注（哪些平台真跑过、哪些只做到编译与单测）。
* **能力探测优先于平台假设**：能用 `--self-check`、网关 `/api/v1/*` 或运行态查询判断的可用性，不用编译期猜测替代；降级与不可用必须显式可见，不得静默失效。
* **数据一律经网关**：桌面端不得直连运行态数据库，不得依赖 `axon-session`、`axon-session-runtime`、`axon-domain`、`rusqlite`（该边界由 `tools/verify-architecture.ps1` 的依赖门禁强制）。

### 阶段 II：质量与合规性检查 (必须执行)

每一项任务必须通过以下自动化校验链：

* **格式检查**：`cargo fmt --all --check`
* **编译检查**：`cargo check --workspace`
* **静态分析**：`cargo clippy --workspace --all-targets -- -D warnings`
* **功能测试**：`cargo test --workspace` (包含针对特定 package 的单元测试与集成测试)
* **架构合规**：`tools/verify-architecture.ps1`
* **代码清理**：`git diff --check` (确保无杂乱空白行与遗留痕迹)

#### 前端门禁质量校验 (axon-desktop 桌面应用，必须执行)

凡改动 `crates/axon-desktop/ui` 或桌面端渲染逻辑，以下各项缺一不得收口；前四条在 `crates/axon-desktop/ui` 目录下执行：

* **依赖安装**：`npm ci`（严格按 `package-lock.json` 还原，禁止用 `npm install` 顺手改锁文件）
* **类型与产物**：`npm run build`（等价 `tsc --noEmit && vite build`）
* **风格检查**：`npm run lint`（等价 `eslint src`）
* **前端单测**：`npm test`（`node --import ./tests/typescript-loader.mjs --test tests/*.test.mjs`）
* **界面契约门禁**：`pwsh -File tools/verify-ui-contract.ps1`，覆盖零 en/em 破折号、无 `window.addEventListener('scroll')`、动画只作用于 transform/opacity、零外部 CDN、主字体白名单、设计令牌对比度实算达 WCAG AA、零第三方设计系统品牌字样，并校验 `ui/dist` 不比 `ui/src` 旧。
* **产物新鲜度 (CRITICAL)**：桌面端二进制把 `ui/dist` 内嵌进产物。**必须先 `npm run build` 再构建 Rust**；只跑 `cargo build -p axon-desktop --release` 不会重建前端，结果是「代码明明改了，界面没变」。
* **行数硬限**：700 行硬限对 `.rs`、`.ts`、`.tsx` 一体生效（`crates`、`apps`、`tools`）；组件级同样按职责拆分，禁止靠堆砌单文件过关。

* **状态管理**：任务完成需显式标记状态，确保任务看板可追踪。

### 阶段 III：文档与 Skill 同步 (必须执行)

**每次改完代码，必须逐项判定 `docs/` 与 `docs/skills/` 是否需要同步更新**；判定为「不需要」时也要在交付说明中给出依据，不得默认跳过。

* **用户面文档**：根 `README.md`、`docs/README.md` 与 `docs/*.md`（CLI 命令、配置键、工具参数契约、守护进程与升级、故障排查等）。
* **开发者规范**：`docs/CONTRIBUTING.md`、`docs/ui-constraints.md` 与 `docs/workbench-design-system.md`（架构、UI 约束、设计系统与贡献指南）。
* **索引同步**：新增、重命名或删除文档时，必须同步 `docs/README.md` 的索引条目；坏链会被门禁直接拦下。
* **官方 Skill SSOT**：`docs/skills/operate-axon/`（`SKILL.md`、`references/*.md`、`agents/openai.yaml`）是随发行包内置的运行时操作手册。凡改动 CLI 命令面与参数、内置工具名与语义、配置键、Skill / Memory / MCP / Daemon / 安装升级行为，**必须同步该 Skill**，并始终以当前 `--help` 与工具回执为事实来源，不写未显示的行为。
* **安装态副本**：安装脚本把 `docs/skills/` 投放到 `$HOME/.axon/skills/`。只改仓库副本运行时读不到：同步后必须重跑安装脚本（或显式同步安装目录），并核对两份内容一致。
* **单一事实来源 (SSOT)**：同一事实只在一处定义，其余位置引用；改动核心逻辑或 API 契约时同步更新对应文档，禁止文档与代码分叉。
* **Skill 内容边界 (CRITICAL)**：`docs/skills/**`（尤其 `operate-axon`）是 **Axon 运行态的唯一事实源**。只允许写运行态事实：当前 `--help` 与子命令参数、可见工具及其回执、配置键与取值来源、HTTP/WS 路由与帧协议、可直接执行的调用方式。**严禁**写任何开发过程性语义：变更说明、新增/修复/更新记录、历史沿革、实施计划与阶段编号、提交号、仓库源码路径、内部实现细节、TODO 与占位。事实与运行态不符时**直接改成正确事实**，不做「本次改了什么」的解释，也不保留旧表述。
* **安装态 Skill 同步 (CRITICAL)**：每次改动 `docs/skills/operate-axon/**` 后，必须检测 `$HOME/.axon/skills/operate-axon` 是否存在：存在则必须按仓库副本同步覆盖同名文件，并逐一核验同名文件 SHA-256 一致；不存在则只保留仓库副本。同步时**保留**安装态 `.revisions/`（Skill Evolution 的运行态数据，不属于随包内容），不得删除或回填。只改仓库副本而未检测安装态副本，视为该任务未完成。
* **文档门禁**：`pwsh -File tools/verify-architecture.ps1`（内含文档校验）与 `pwsh -File tools/verify-docs.ps1` 必须通过：UTF-8 无 BOM、Markdown 零坏链、术语一致（`.mcp.json` 写法、`UTF-8 without BOM` 口径、不得依赖 GitNexus）。

### 阶段 IV：构建与覆盖安装 (发布约束)

稳定 launcher 与不可变版本树组成唯一的发布事实，安装根为 `$HOME/.axon/bin`（Windows 为 `%USERPROFILE%\.axon\bin`），可用 `AXON_INSTALL_ROOT` 覆盖：

```text
$HOME/.axon/bin/
├── axon.cmd                  # 稳定 PATH 入口（POSIX 为同名软链），转发给 launcher
├── current.json              # 指向当前生效的版本目录
├── launcher/axon-launcher.exe
└── versions/<版本号>/{axon.exe, axon-desktop.exe}
```

* **唯一安装入口**：源码安装走 `scripts/install-from-source.ps1`，发行包安装走 `scripts/install.ps1` / `scripts/install.sh`，两者都写入上述版本树。**严禁**另起覆盖安装机制或绕过 launcher 发布。
* **严禁 `cargo install` (CRITICAL)**：`cargo install --path crates/axon-cli` 及任何 `cargo install` 都会把二进制落到 `$env:CARGO_HOME\bin`（默认 `$HOME/.cargo/bin`），造成版本树分裂与 PATH 遮蔽：`axon` 指向陈旧二进制，CLI 与 daemon 版本不一致。源码安装脚本已主动清理该目录下的 `axon.exe` 与 `axon-desktop.exe`，不得再把它们放回去。
* **严禁手工投放**：不得把 `target/**` 产物复制到 cargo 的 bin 目录、安装根目录根部或任何自建目录；唯一例外是上一条「纯前端改动快速通道」允许覆盖版本目录内的 `axon-desktop.exe`。除此之外，版本目录只能由安装脚本原子替换。Windows 上编译产物无法覆盖正在运行的映像，覆盖前必须先停对应进程。
* **前端先于后端**：整链顺序固定为 `npm ci` → `npm run build` → `cargo build --release --locked` → 覆盖安装。
* **纯前端改动快速通道（允许，边界严格）**：改动只落在 `crates/axon-desktop/ui/**`（不含任何 Rust 侧改动）时，不必重跑整条安装链，按最小闭环更新本机应用版本：

```powershell
# 1) 重建前端产物（在 crates/axon-desktop/ui 下执行；依赖未变时无需 npm ci）
npm run build

# 2) 只重编桌面端 release 产物（在仓库根执行）
cargo build --release --locked -p axon-desktop

# 3) 从 current.json 解析安装态版本目录，并停桌面端进程释放映像文件锁
$installRoot = if ($env:AXON_INSTALL_ROOT) { $env:AXON_INSTALL_ROOT } else { Join-Path $HOME '.axon\bin' }
$versionId = (Get-Content (Join-Path $installRoot 'current.json') -Raw | ConvertFrom-Json).versionId
$built = 'target\release\axon-desktop.exe'
$target = Join-Path $installRoot "versions\$versionId\axon-desktop.exe"
Get-Process axon-desktop -ErrorAction SilentlyContinue | Stop-Process -Force

# 4) 覆盖并核验：与本次构建产物逐字节一致（哈希必须为 True）
Copy-Item $built $target -Force
"installed: {0}  hash-match: {1}" -f (Get-Item $target).LastWriteTime, ((Get-FileHash $built).Hash -eq (Get-FileHash $target).Hash)

# 5) 经安装根稳定入口重启桌面端，核验界面已更新
```

构建前确认 `cargo` 与 `npm` 在当前 shell 可用；不在 PATH 时先补上工具链 bin 目录，不得改动全局 PATH 优先级。

  * 覆盖目标必须取自 `current.json` 的 `versionId`，即 `$HOME/.axon/bin/versions/<versionId>/axon-desktop.exe`；**不得**新建版本目录、不得改 `current.json`、不得改同目录的 `axon.exe`、不得动 Daemon 与 CLI。
  * 核验（缺一不可）：版本目录内 `axon-desktop.exe` 的 `LastWriteTime` 不早于本次构建产物，且与 `target/release/axon-desktop.exe` 的 SHA-256 一致；`current.json` 前后不变；桌面端可正常启动并显示新界面。
  * 证据形式：构建耗时行（`Finished 'release' profile ... in <时长>`）、覆盖后版本目录内二进制的时间戳、覆盖完成时间，三者一并登记。
  * 触及任何 Rust 侧文件或安装树结构时，本条立即失效，必须回到完整安装链（`scripts/install-from-source.ps1`）。
* **安装后必须核验**：`current.json` 指向的版本目录内 `axon.exe` 与 `axon-desktop.exe` 同时存在；`Get-Command axon` 解析到安装根的稳定入口而非 cargo 目录；cargo 的 bin 目录下无 `axon*.exe` 残留；`axon --version` 与本次构建一致；Daemon 处于运行态。
* **MSI 路径**：MSI 构建与安装说明见 `installer/windows/README.md`，必须从仓库根目录执行；MSI 不得写 `%USERPROFILE%\.axon`。

### 阶段 V：真机验证 (安装态与桌面 CDP)

真机证据只能来自**安装态二进制与真实窗口**；离线门禁、单元测试、Mock 与静态截图一律不得冒充真机证据。

* **验证对象**：`$HOME/.axon/bin/versions/<版本号>/` 下的安装态二进制，并与 `target/release/` 产物核对 SHA-256，证明验证版本与提交对应。直接跑源码树 `target/**` 不算真机。
* **前置动作**：停 Daemon 与桌面端（释放映像文件锁）→ 覆盖安装 → 重启 Daemon → 经安装根稳定入口（`axon desktop` / launcher）启动，禁止绕过 launcher 直连版本目录。
* **桌面界面精验走 CDP**（唯一可重复、可取证的手段）：
  * 启动前设置 `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9333'`，再启动桌面端；
  * 复用仓库既有辅助脚本（默认端口 9333，可用 `-Port` 覆盖）：`tools/cdp-eval.ps1` 做 `Runtime.evaluate` 一次性求值读回 DOM 与状态；`tools/cdp-click.ps1` 先滚动到可见再派发真实 `Input.dispatchMouseEvent` 的 mousedown/mouseup/click；`tools/cdp-type.ps1` 做真实文本输入，`-Submit` 追加 Enter；
  * **判定看输出文本，不看退出码 (CRITICAL)**：脚本在 CDP 不可达、无 page target、未命中元素、元素中心点被遮挡时输出 `ERR:...` 并仍以退出码 0 收尾，成功输出 `OK:...`。必须断言 `OK:` 前缀，否则等于没验证；
  * **收尾清理**：验证结束必须关闭调试端口（新开 shell 或清空该环境变量），不得把调试端口带进日常使用。
* **浏览器等价验证**：界面是标准 Web 应用，可 `npm run build` 后用 `vite preview`（或直接托管 `ui/dist`）在 Chromium 中经同一套 CDP 手段取证：视口宽度、`prefers-reduced-motion` 降级、性能轨迹与帧率。它与真机窗口消费同一批事件帧，结论可迁移。
* **机器可读证据**：`axon-desktop.exe --self-check` 输出单行 JSON 并以退出码表示成败（读 → 校验 → 写 → 复读往返）。
* **隔离与零副作用**：真机验证期间不得改动真实用户配置 `$HOME/.axon/config.toml`（前后 SHA-256 必须一致）；写路径验证一律在 `AXON_RUNTIME_ROOT` 指向的临时根内进行；不得读取或推送历史通知与真实会话数据。
* **受保护的真机场景**：`tools/verify-daemon-restart.sh` 缺少 `REAL_DEVICE=1` 时直接拒绝运行，离线报告不得冒充真机证据。
* **必交证据**：安装态版本与提交号、关键步骤原始回执（CDP 的 `OK:` 行、自检 JSON、`axon daemon status`）与失败项如实登记；未取得真机证据的条目一律标注「未真机验证」，禁止用测试覆盖替代后仍宣称真机通过。

---

## 3. 提交与版本控制规范

* **AGENTS.md 纳入版本控制**：本文件是团队共享的开发准则，必须随仓库提交（已从 `.gitignore` 移除忽略规则）。修改本文件时与代码同批提交，不长期留在工作区。
* **禁止提交**：严禁提交环境现场文件与运行态产物，包括但不限于 `.claude/`、`CLAUDE.md`、`.env`（`.env.example` 除外）、`data/`、`target/`、构建缓存、日志、`*.sqlite3`、安装态二进制、个人 IDE 配置与任何凭据文件。
* **提交内容**：仅包含实现任务所需的源码、配置、文档与对应的测试用例。
* **提交前敏感信息自检（必须执行）**：任何提交前先扫查暂存区，三条扫查必须**零命中**，命中即修完再提交。

```powershell
# 1) 本机绝对路径（用户目录、盘符、家目录）
git diff --cached -U0 | Select-String -Pattern '(?i)[A-Z]:\\{1,2}(Users|Documents|Desktop)\\(?!example\\|tester\\|user\\|<)|/Users/(?!example/|tester/|user/)[a-z0-9._-]+|/home/(?!example/|tester/|user/)[a-z0-9._-]+'
# 2) 凭据与令牌（真实密钥一律不得出现）
git diff --cached -U0 | Select-String -Pattern '(?i)(sk-[A-Za-z0-9]{16,}|ghp_[A-Za-z0-9]{16,}|xox[baprs]-[A-Za-z0-9-]{10,}|(api[_-]?key|api[_-]?token|secret|password)\s*[=:]\s*[A-Za-z0-9._-]{12,})'
# 3) 私钥与证书
git diff --cached -U0 | Select-String -Pattern 'BEGIN [A-Z ]*PRIVATE KEY'
# 4) 文件清单逐项复核
git diff --cached --stat
```

  * 路径一律写成 `$HOME`、`$env:USERPROFILE`、`%USERPROFILE%` 这类环境变量或仓库相对路径，不写具体机器位置（见 §1「路径无关性」）。
  * 文档与 Skill 中出现的令牌、地址、路径一律用 `<token>`、`<path>` 等占位符，绝不写真实值。
  * 提交信息用中文，不出现 en/em 破折号；不使用 `--force` 推送，推送被拒先 `pull --rebase`。

---

## 4. 任务状态追踪模板 (Task Template)

每当您发起新的 AXON 任务时，请使用以下格式进行状态汇报：

```markdown
### [AXON-ID]: 任务标题
- **状态**: [进行中/待审核/已完成]
- **任务目标**: (明确本任务的交付成果)

```

---

## 5. 最终验收标准

本项目的目标达成即意味着：

1. **架构完备**：代码结构完全符合洋葱架构分层，依赖方向正确。
2. **零冗余/零技术债务**：不存在遗留的调试代码、死代码或 Mock 逻辑。
3. **生产就绪**：满足所有自动化测试用例，且具备生产级的异常处理能力。
4. **门禁全绿**：Rust 校验链与前端门禁（`npm ci` / `npm run build` / `npm run lint` / `npm test` / `tools/verify-ui-contract.ps1`）全部通过，且 `ui/dist` 不旧于前端源码。
5. **文档与 Skill 一致**：`docs/` 与 `docs/skills/` 已按阶段 III 判定并同步（含索引与安装态副本），文档门禁通过，所有交付物均反映在最新版本的技术蓝图与 PRD 中。
6. **安装树唯一**：交付只存在于 `$HOME/.axon/bin` 版本树，cargo 的 bin 目录与任何自建目录均无 Axon 二进制残留，`axon` 稳定入口解析正确。
7. **真机验证闭环**：按阶段 V 在安装态真机完成验证并留有可复核证据（桌面端经 CDP 的 `OK:` 回执）；未验证项如实登记，不以测试或 Mock 冒充。

---