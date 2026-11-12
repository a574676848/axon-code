<#
.SYNOPSIS
    Axon 记忆增强 M0-1 基线核验脚本。

.DESCRIPTION
    校验 README v1.0 中引用的 Axon 代码锚点是否仍然成立，输出「存在 / 已漂移 / 缺失」三态表；
    同时输出 context/ 目录实际文件清单与 MemoryHybridQuery 字段集合，供 M0-2 事实文档核对。

    三态判定规则：
      存在     文件存在，且期望行号处的文本命中锚点指纹；
      已漂移   文件存在，但期望行号不再命中，指纹在文件其它行命中，报告新行号；
      缺失     文件不存在，或文件内已找不到该指纹。

    本脚本只读，不修改仓库任何文件；退出码固定为 0（存在漂移时以表格与摘要体现）。

.EXAMPLE
    pwsh -NoProfile -File scripts/memory-baseline-check.ps1
#>

[CmdletBinding()]
param(
    # 仓库根目录；默认取脚本所在目录的上一级。
    [string]$RepoRoot = (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))
)

$ErrorActionPreference = 'Stop'
try {
    [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
} catch {
    # 控制台不支持切换编码时忽略：表格仍会输出。
}

# README v1.0 引用的代码锚点。Pattern 为该行必须命中的指纹（正则）。
$anchors = @(
    @{ Id = 'A01'; Path = 'crates/axon-memory/src/retrieval/retrieval.rs'; Line = 10;  Pattern = 'MEMORY_HYBRID_CANDIDATE_MULTIPLIER' }
    @{ Id = 'A02'; Path = 'crates/axon-memory/src/retrieval/retrieval.rs'; Line = 22;  Pattern = 'pub struct MemoryHybridQuery' }
    @{ Id = 'A03'; Path = 'crates/axon-memory/src/retrieval/retrieval.rs'; Line = 98;  Pattern = 'pub fn hybrid_search' }
    @{ Id = 'A04'; Path = 'crates/axon-memory/src/store/store.rs';         Line = 243; Pattern = 'CREATE TABLE IF NOT EXISTS memory_records' }
    @{ Id = 'A05'; Path = 'crates/axon-memory/src/store/store.rs';         Line = 298; Pattern = 'user_version' }
    @{ Id = 'A06'; Path = 'crates/axon-memory/src/store/store_supersession.rs'; Line = 17; Pattern = 'CREATE TABLE IF NOT EXISTS memory_supersessions' }
    @{ Id = 'A07'; Path = 'crates/axon-memory/src/projection/vector_projection.rs'; Line = 57; Pattern = 'CREATE TABLE IF NOT EXISTS memory_vector_projections' }
    @{ Id = 'A08'; Path = 'crates/axon-memory/src/usage/quality.rs';       Line = 81;  Pattern = 'CREATE TABLE IF NOT EXISTS memory_runtime_quality' }
    @{ Id = 'A09'; Path = 'crates/axon-persona/src/infrastructure/sqlite_persona_store.rs'; Line = 1;  Pattern = 'persona\.sqlite3' }
    @{ Id = 'A10'; Path = 'crates/axon-persona/src/infrastructure/sqlite_persona_store.rs'; Line = 24; Pattern = 'PERSONA_SCHEMA_IDENTITY' }
    @{ Id = 'A11'; Path = 'crates/axon-persona/src/infrastructure/sqlite_persona_store.rs'; Line = 28; Pattern = 'CREATE TABLE IF NOT EXISTS persona_state' }
    @{ Id = 'A12'; Path = 'crates/axon-agent-runtime/src/runtime/self_improvement/self_iteration.rs'; Line = 19; Pattern = '\S' }
    @{ Id = 'A13'; Path = 'crates/axon-cli/src/run_commands/run_self_iteration_subagent.rs'; Line = 32; Pattern = '\S' }
)

# README §2.3 声明存在的 context/ 文件。
$expectedContextFiles = @(
    'context_assembler.rs'
    'agent_context.rs'
    'agent_context_memory.rs'
    'agent_context_memory_layers.rs'
    'context_budget.rs'
    'environment_context.rs'
    'focus_resolver.rs'
    'token_estimator.rs'
    'window_rollover.rs'
    'agent_context/sections.rs'
    'agent_context/command_fragments.rs'
    'agent_context/history_fragments.rs'
)

function Resolve-RepoPath {
    param([string]$Relative)
    return (Join-Path $RepoRoot ($Relative -replace '/', [System.IO.Path]::DirectorySeparatorChar))
}

function Test-Anchor {
    param([hashtable]$Anchor)

    $full = Resolve-RepoPath $Anchor.Path
    $result = [ordered]@{
        Id     = $Anchor.Id
        Path   = $Anchor.Path
        Line   = $Anchor.Line
        State  = '缺失'
        Detail = ''
    }
    if (-not (Test-Path -LiteralPath $full)) {
        $result.Detail = '文件不存在'
        return [pscustomobject]$result
    }

    $lines = @(Get-Content -LiteralPath $full -Encoding UTF8)
    if ($Anchor.Line -ge 1 -and $Anchor.Line -le $lines.Count) {
        $current = $lines[$Anchor.Line - 1]
        if ($current -match $Anchor.Pattern) {
            $result.State = '存在'
            $result.Detail = $current.Trim()
            return [pscustomobject]$result
        }
    }

    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match $Anchor.Pattern) {
            $result.State = '已漂移'
            $result.Line = $i + 1
            $result.Detail = $lines[$i].Trim()
            return [pscustomobject]$result
        }
    }

    $result.Detail = '指纹在文件内未命中'
    return [pscustomobject]$result
}

Write-Host '== Axon 记忆增强基线核验 =='
Write-Host ("仓库根目录：{0}" -f $RepoRoot)
Write-Host ''

Write-Host '-- 1. 代码锚点三态表 --'
$anchorResults = foreach ($anchor in $anchors) { Test-Anchor $anchor }
$anchorResults | Format-Table -AutoSize -Property Id, State, Line, Path, Detail | Out-String -Width 200 | Write-Host

$present = @($anchorResults | Where-Object { $_.State -eq '存在' }).Count
$drifted = @($anchorResults | Where-Object { $_.State -eq '已漂移' }).Count
$missing = @($anchorResults | Where-Object { $_.State -eq '缺失' }).Count
Write-Host ("锚点合计 {0} 项：存在 {1} 项 / 已漂移 {2} 项 / 缺失 {3} 项" -f $anchorResults.Count, $present, $drifted, $missing)
Write-Host ''

Write-Host '-- 2. context/ 文件清单核对（README §2.3） --'
$contextRoot = Resolve-RepoPath 'crates/axon-agent-runtime/src/core/context'
if (-not (Test-Path -LiteralPath $contextRoot)) {
    Write-Host ("[缺失] context 目录不存在：{0}" -f $contextRoot)
} else {
    $actual = @(Get-ChildItem -LiteralPath $contextRoot -Recurse -File -Filter '*.rs' |
        ForEach-Object { $_.FullName.Substring($contextRoot.Length + 1) -replace '\\', '/' } |
        Sort-Object)
    Write-Host ("实际文件数：{0}" -f $actual.Count)
    foreach ($expected in $expectedContextFiles) {
        $state = if ($actual -contains $expected) { '存在' } else { '缺失' }
        Write-Host ("  [{0}] {1}" -f $state, $expected)
    }
    $unexpected = @($actual | Where-Object { $expectedContextFiles -notcontains $_ })
    if ($unexpected.Count -gt 0) {
        Write-Host ("  另有无声明文件 {0} 个：{1}" -f $unexpected.Count, ($unexpected -join ', '))
    }
}
Write-Host ''

Write-Host '-- 3. MemoryHybridQuery 字段集合 --'
$retrievalPath = Resolve-RepoPath 'crates/axon-memory/src/retrieval/retrieval.rs'
if (-not (Test-Path -LiteralPath $retrievalPath)) {
    Write-Host '[缺失] retrieval.rs 不存在'
} else {
    $text = Get-Content -LiteralPath $retrievalPath -Raw -Encoding UTF8
    $structMatch = [regex]::Match($text, 'pub struct MemoryHybridQuery\s*\{(?<body>[^}]*)\}')
    if (-not $structMatch.Success) {
        Write-Host '[缺失] 未解析到 MemoryHybridQuery 结构体'
    } else {
        $fields = [regex]::Matches($structMatch.Groups['body'].Value, '(?m)^\s*pub\s+(?<name>[A-Za-z0-9_]+)\s*:') |
            ForEach-Object { $_.Groups['name'].Value }
        Write-Host ("字段数：{0}" -f @($fields).Count)
        foreach ($field in $fields) { Write-Host ("  - {0}" -f $field) }
        foreach ($required in @('kinds', 'time_range')) {
            $state = if (@($fields) -contains $required) { '存在' } else { '缺失' }
            Write-Host ("  [{0}] 计划新增字段 {1}" -f $state, $required)
        }
    }
}
Write-Host ''

Write-Host '核验完成（exit 0）。'
exit 0
