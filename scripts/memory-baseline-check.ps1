<#
.SYNOPSIS
    Axon 记忆增强 · M0-1 基线核验脚本

.DESCRIPTION
    校验 E:\zbg\axon-memory-plan\README.md 引用的代码锚点是否仍然成立。
    输出三态：
      [OK   ] 存在，且行号与文档一致
      [DRIFT] 符号存在但行号已漂移（给出新行号）
      [MISS ] 文件或符号不存在
    只读脚本，不修改仓库任何文件。

.PARAMETER RepoPath
    Axon 仓库根目录，默认 E:\zbg\axon-code

.PARAMETER Strict
    缺失项 > 0 时返回退出码 1；默认容忍至多 3 项缺失（文档标注"待补"的项不计入）

.EXAMPLE
    pwsh -NoProfile -File scripts/memory-baseline-check.ps1
    pwsh -NoProfile -File scripts/memory-baseline-check.ps1 -RepoPath E:\zbg\axon-code -Strict
#>
[CmdletBinding()]
param(
    [string]$RepoPath = 'E:\zbg\axon-code',
    [switch]$Strict
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $RepoPath)) {
    Write-Host "FATAL: 仓库路径不存在: $RepoPath" -ForegroundColor Red
    exit 2
}

$RepoPath = (Resolve-Path -LiteralPath $RepoPath).Path

# ---------------------------------------------------------------- 角色色
function Write-Ok      { param($s) Write-Host $s -ForegroundColor DarkGreen }
function Write-Drift   { param($s) Write-Host $s -ForegroundColor Yellow }
function Write-Missing { param($s) Write-Host $s -ForegroundColor Red }
function Write-Head    { param($s) Write-Host ""; Write-Host $s -ForegroundColor Cyan }

# ---------------------------------------------------------------- 锚点定义
# Expect = 0 表示只校验符号存在，不校验行号
# Optional = $true 表示文档已标注"待补/可能不存在"，缺失不算失败
$checks = @(
    # ---- 记忆存储 ----
    @{ Id='S1'; File='crates/axon-memory/src/store/store.rs';                        Pattern='memory_records';                    Expect=243;  Optional=$false; Note='记忆主表' }
    @{ Id='S2'; File='crates/axon-memory/src/store/store.rs';                        Pattern='user_version';                      Expect=298;  Optional=$false; Note='schema 版本' }
    @{ Id='S3'; File='crates/axon-memory/src/store/store.rs';                        Pattern='ttl_expires_at_ms';                 Expect=264;  Optional=$false; Note='TTL 字段' }
    @{ Id='S4'; File='crates/axon-memory/src/store/store_supersession.rs';           Pattern='memory_supersessions';              Expect=17;   Optional=$false; Note='覆盖链表' }
    @{ Id='S5'; File='crates/axon-memory/src/projection/vector_projection.rs';       Pattern='memory_vector_projections';         Expect=57;   Optional=$false; Note='向量投影表' }

    # ---- 检索 ----
    @{ Id='R1'; File='crates/axon-memory/src/retrieval/retrieval.rs';                Pattern='MemoryHybridQuery';                 Expect=22;   Optional=$false; Note='混合检索入参' }
    @{ Id='R2'; File='crates/axon-memory/src/retrieval/retrieval.rs';                Pattern='MEMORY_HYBRID_CANDIDATE_MULTIPLIER';Expect=10;   Optional=$false; Note='候选扩召回倍数' }
    @{ Id='R3'; File='crates/axon-memory/src/retrieval/retrieval.rs';                Pattern='fn hybrid_search';                  Expect=98;   Optional=$false; Note='检索入口' }

    # ---- 人格 ----
    @{ Id='P1'; File='crates/axon-persona/src/infrastructure/sqlite_persona_store.rs'; Pattern='persona_state';                  Expect=28;   Optional=$false; Note='人格状态表' }
    @{ Id='P2'; File='crates/axon-persona/src/infrastructure/sqlite_persona_store.rs'; Pattern='axon-persona';                  Expect=24;   Optional=$false; Note='schema 身份' }

    # ---- 上下文装配 ----
    @{ Id='C1'; File='crates/axon-agent-runtime/src/core/context/context_assembler.rs';        Pattern='';  Expect=0; Optional=$false; Note='装配入口' }
    @{ Id='C2'; File='crates/axon-agent-runtime/src/core/context/context_budget.rs';           Pattern='';  Expect=0; Optional=$false; Note='预算' }
    @{ Id='C3'; File='crates/axon-agent-runtime/src/core/context/agent_context_memory_layers.rs'; Pattern=''; Expect=0; Optional=$false; Note='记忆分层' }
    @{ Id='C4'; File='crates/axon-agent-runtime/src/core/context/agent_context/sections.rs';   Pattern='sha256_hex'; Expect=0; Optional=$false; Note='section 构造与预算映射（README 误标“待补”，实际存在）' }
    @{ Id='C5'; File='crates/axon-agent-runtime/src/core/context/environment_context.rs';      Pattern='';  Expect=0; Optional=$false; Note='环境上下文' }
    @{ Id='C6'; File='crates/axon-agent-runtime/src/core/context/agent_context/command_fragments.rs'; Pattern=''; Expect=0; Optional=$false; Note='命令片段' }

    # ---- prompt cache（M7 的真实底座，README 误判为“待补”） ----
    @{ Id='K1'; File='crates/axon-agent-runtime/src/core/context/prompt_cache/prompt_cache.rs';       Pattern=''; Expect=0; Optional=$false; Note='缓存主体' }
    @{ Id='K2'; File='crates/axon-agent-runtime/src/core/context/prompt_cache/prompt_cache_plan.rs';  Pattern=''; Expect=0; Optional=$false; Note='分区计划' }
    @{ Id='K3'; File='crates/axon-agent-runtime/src/core/context/prompt_cache/prompt_cache_wire.rs';  Pattern=''; Expect=0; Optional=$false; Note='线格式转换' }
    @{ Id='K4'; File='crates/axon-agent-runtime/src/core/context/prompt_cache/prompt_cache_tool_wire.rs'; Pattern=''; Expect=0; Optional=$false; Note='工具消息线格式' }
    @{ Id='K5'; File='crates/axon-agent-runtime/src/core/context/prompt_cache/prompt_cache_anchor_tests.rs'; Pattern=''; Expect=0; Optional=$false; Note='锚点测试（可能已覆盖 golden 断言）' }

    # ---- 工具与反思 ----
    @{ Id='T1'; File='crates/axon-agent-runtime/src/core/tools/builtin_memory_search_tool.rs'; Pattern='memory_search'; Expect=0; Optional=$false; Note='memory_search 工具' }
    @{ Id='I1'; File='crates/axon-agent-runtime/src/runtime/self_improvement/self_iteration.rs';                 Pattern='memory_update';     Expect=19; Optional=$false; Note='反思写入模式' }
    @{ Id='I2'; File='crates/axon-cli/src/run_commands/run_self_iteration_subagent.rs';                          Pattern='semantic_salience'; Expect=32; Optional=$false; Note='语义显著度门控（实测在 axon-cli，不在 agent-runtime）' }
)

# ---------------------------------------------------------------- 仓库环境
Write-Head '=== 0. 仓库环境 ==='
Push-Location $RepoPath
try {
    $branch = (& git rev-parse --abbrev-ref HEAD 2>$null)
    $head   = (& git log -1 --format='%h %ad %s' --date=short 2>$null)
    $dirty  = (& git status --porcelain 2>$null | Measure-Object -Line).Lines
    Write-Host ("RepoPath : {0}" -f $RepoPath)
    Write-Host ("Branch   : {0}" -f $branch)
    Write-Host ("HEAD     : {0}" -f $head)
    Write-Host ("Dirty    : {0} file(s)" -f $dirty)
}
finally {
    Pop-Location
}

# ---------------------------------------------------------------- 逐项核验
Write-Head '=== 1. 锚点核验 ==='

$results = foreach ($c in $checks) {
    $full = Join-Path $RepoPath $c.File
    if (-not (Test-Path -LiteralPath $full)) {
        [pscustomobject]@{ Id=$c.Id; State='MISS'; Detail='文件不存在'; Actual=0; Expect=$c.Expect; File=$c.File; Note=$c.Note; Optional=$c.Optional }
        continue
    }
    if ([string]::IsNullOrWhiteSpace($c.Pattern)) {
        [pscustomobject]@{ Id=$c.Id; State='OK'; Detail='文件存在'; Actual=0; Expect=$c.Expect; File=$c.File; Note=$c.Note; Optional=$c.Optional }
        continue
    }
    $hit = Select-String -LiteralPath $full -Pattern $c.Pattern -SimpleMatch -List
    if (-not $hit) {
        [pscustomobject]@{ Id=$c.Id; State='MISS'; Detail=("符号未找到: {0}" -f $c.Pattern); Actual=0; Expect=$c.Expect; File=$c.File; Note=$c.Note; Optional=$c.Optional }
        continue
    }
    $actual = $hit.LineNumber
    $state  = if ($c.Expect -le 0 -or $actual -eq $c.Expect) { 'OK' } else { 'DRIFT' }
    [pscustomobject]@{ Id=$c.Id; State=$state; Detail=''; Actual=$actual; Expect=$c.Expect; File=$c.File; Note=$c.Note; Optional=$c.Optional }
}

foreach ($r in $results) {
    $shortFile = $r.File -replace '^crates/', '' -replace '^axon-', 'axon-'
    switch ($r.State) {
        'OK' {
            if ($r.Expect -gt 0) {
                Write-Ok ("[OK   ] {0}  {1}:{2}  {3}" -f $r.Id, $shortFile, $r.Actual, $r.Note)
            } else {
                Write-Ok ("[OK   ] {0}  {1}  {2}" -f $r.Id, $shortFile, $r.Note)
            }
        }
        'DRIFT' {
            Write-Drift ("[DRIFT] {0}  {1}  文档 {2} -> 实际 {3}  {4}" -f $r.Id, $shortFile, $r.Expect, $r.Actual, $r.Note)
        }
        'MISS' {
            $tag = if ($r.Optional) { '(可选)' } else { '' }
            Write-Missing ("[MISS ] {0}  {1}  {2}  {3}{4}" -f $r.Id, $shortFile, $r.Note, $r.Detail, $tag)
        }
    }
}

$okCount     = @($results | Where-Object { $_.State -eq 'OK'     }).Count
$driftCount  = @($results | Where-Object { $_.State -eq 'DRIFT'  }).Count
$missCount   = @($results | Where-Object { $_.State -eq 'MISS' -and -not $_.Optional }).Count
$missOptCnt  = @($results | Where-Object { $_.State -eq 'MISS' -and $_.Optional      }).Count

Write-Head '=== 2. 汇总 ==='
Write-Host ("OK={0}  DRIFT={1}  MISS(必需)={2}  MISS(可选)={3}  合计={4}" -f $okCount, $driftCount, $missCount, $missOptCnt, $results.Count)

# ---------------------------------------------------------------- 上下文目录清单
Write-Head '=== 3. context/ 目录实际清单（README §2.3 待补项的输入） ==='
$ctxDir = Join-Path $RepoPath 'crates/axon-agent-runtime/src/core/context'
if (Test-Path -LiteralPath $ctxDir) {
    $files = Get-ChildItem -LiteralPath $ctxDir -File | Sort-Object Name
    $prod  = $files | Where-Object { $_.Name -notmatch '_tests\.rs$' }
    $tests = $files | Where-Object { $_.Name -match '_tests\.rs$' }
    Write-Host ("生产文件 {0} 个 / 测试文件 {1} 个" -f $prod.Count, $tests.Count)
    foreach ($f in $prod) {
        Write-Host ("  - {0,-46} {1,8} B" -f $f.Name, $f.Length)
    }
    $subDirs = Get-ChildItem -LiteralPath $ctxDir -Directory | Sort-Object Name
    foreach ($d in $subDirs) {
        $subFiles = Get-ChildItem -LiteralPath $d.FullName -File
        $subSize  = ($subFiles | Measure-Object -Property Length -Sum).Sum
        Write-Host ("  [{0}/] {1} 个文件, {2:N0} B" -f $d.Name, $subFiles.Count, $subSize)
    }
} else {
    Write-Missing "context/ 目录不存在: $ctxDir"
}

# ---------------------------------------------------------------- 关键结构快照
Write-Head '=== 4. MemoryHybridQuery 字段快照（M1-1 的输入） ==='
$retrieval = Join-Path $RepoPath 'crates/axon-memory/src/retrieval/retrieval.rs'
if (Test-Path -LiteralPath $retrieval) {
    $lines = Get-Content -LiteralPath $retrieval
    $startIdx = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match 'struct\s+MemoryHybridQuery') { $startIdx = $i; break }
    }
    if ($startIdx -ge 0) {
        $depth = 0
        $printed = 0
        for ($i = $startIdx; $i -lt $lines.Count -and $printed -lt 40; $i++) {
            $line = $lines[$i]
            Write-Host ("  {0,5}: {1}" -f ($i + 1), $line)
            $printed++
            $depth += ([regex]::Matches($line, '\{')).Count
            $depth -= ([regex]::Matches($line, '\}')).Count
            if ($depth -le 0 -and $i -gt $startIdx) { break }
        }
    } else {
        Write-Missing '  未找到 struct MemoryHybridQuery'
    }
} else {
    Write-Missing "  retrieval.rs 不存在"
}

# ---------------------------------------------------------------- 退出码
Write-Head '=== 5. 结论 ==='
$limit = if ($Strict) { 0 } else { 3 }
if ($missCount -gt $limit) {
    Write-Missing ("必需锚点缺失 {0} 项，超过容忍上限 {1} —— 文档需先修正再开工" -f $missCount, $limit)
    exit 1
}
if ($driftCount -gt 0) {
    Write-Host ("存在 {0} 处行号漂移，建议把 README 的行号更新为本次实测值后再派工。" -f $driftCount) -ForegroundColor Yellow
}
Write-Host '核验完成。'
exit 0
