$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')

function Assert-PwshRuntime {
    if ($PSVersionTable.PSVersion.Major -lt 7) {
        throw "verify-tui-smoke requires PowerShell 7+, actual=$($PSVersionTable.PSVersion)"
    }
}

function Invoke-AgentTuiJsonSmoke {
    param(
        [hashtable] $Environment = @{}
    )

    $previous = @{}
    $managedKeys = @(
        'TERM',
        'TERM_PROGRAM',
        'WT_SESSION',
        'WSL_DISTRO_NAME',
        'WEZTERM_EXECUTABLE',
        'ALACRITTY_SOCKET',
        'GHOSTTY_RESOURCES_DIR',
        'ITERM_SESSION_ID',
        'TMUX',
        'STY',
        'SSH_CONNECTION',
        'SSH_TTY',
        'VSCODE_INJECTION',
        'AXON_TUI_SSH_SLOW_LINK'
    )
    if ($Environment.Count -gt 0) {
        foreach ($key in $managedKeys) {
            $previous[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
            [Environment]::SetEnvironmentVariable($key, $null, 'Process')
        }
        foreach ($key in $Environment.Keys) {
            [Environment]::SetEnvironmentVariable($key, [string] $Environment[$key], 'Process')
        }
    }
    try {
        $output = cargo run --quiet -p axon-cli -- --format json
        if ($LASTEXITCODE -ne 0) {
            throw "axon-cli JSON smoke failed with exit code $LASTEXITCODE"
        }
        return $output | ConvertFrom-Json
    }
    finally {
        if ($Environment.Count -gt 0) {
            foreach ($key in $managedKeys) {
                [Environment]::SetEnvironmentVariable($key, $previous[$key], 'Process')
            }
        }
    }
}

function Assert-TerminalFixture {
    param(
        [string] $Name,
        [hashtable] $Environment,
        [string] $ExpectedProfile,
        [string] $ExpectedHyperlinkMode,
        [string] $ExpectedMouseMode
    )

    $fixtureReport = Invoke-AgentTuiJsonSmoke -Environment $Environment
    $compat = $fixtureReport.terminalCompatibility
    if ($null -eq $compat) {
        throw "$Name terminal compatibility snapshot missing"
    }
    if ($compat.profile -ne $ExpectedProfile) {
        throw "$Name expected profile=$ExpectedProfile actual=$($compat.profile)"
    }
    if ($compat.hyperlinkMode -ne $ExpectedHyperlinkMode) {
        throw "$Name expected hyperlink=$ExpectedHyperlinkMode actual=$($compat.hyperlinkMode)"
    }
    if ($compat.mouseMode -ne $ExpectedMouseMode) {
        throw "$Name expected mouseMode=$ExpectedMouseMode actual=$($compat.mouseMode)"
    }
    Write-Output "terminal fixture: $Name profile=$($compat.profile) hyperlink=$($compat.hyperlinkMode) copy=$($compat.copyMode) mouseMode=$($compat.mouseMode) replayRows=$($compat.replayMaxRows)"
}

function Assert-AgentTuiAbilityMatrix {
    param(
        [object] $Report
    )

    $matrix = $Report.abilityMatrix
    if ($null -eq $matrix) {
        throw 'abilityMatrix missing from Agent TUI launch report'
    }
    if ($matrix.totalCount -lt 95) {
        throw "ability matrix totalCount too small: $($matrix.totalCount)"
    }
    if ($matrix.smokeCoveredCount -lt 80) {
        throw "ability matrix smokeCoveredCount too small: $($matrix.smokeCoveredCount)"
    }
    if ($null -eq $matrix.abilities -or $matrix.abilities.Count -ne $matrix.totalCount) {
        throw "ability matrix count mismatch: abilities=$($matrix.abilities.Count) total=$($matrix.totalCount)"
    }

    $requiredCategories = @(
        'input',
        'attachment_mention',
        'queue_steering',
        'slash_command',
        'composer_popup',
        'dock',
        'transcript_cell',
        'runtime_projection',
        'live_cell_mutation',
        'system_catalog'
    )
    $categoryNames = @($matrix.categories | ForEach-Object { $_.category })
    foreach ($category in $requiredCategories) {
        if ($categoryNames -notcontains $category) {
            throw "ability category missing: $category"
        }
    }

    $abilityById = @{}
    foreach ($ability in $matrix.abilities) {
        if ([string]::IsNullOrWhiteSpace($ability.id)) {
            throw 'ability id missing in abilityMatrix'
        }
        if ($abilityById.ContainsKey($ability.id)) {
            throw "duplicate ability id: $($ability.id) target=$($ability.targetEntry)"
        }
        foreach ($field in @('category', 'oldEntry', 'targetEntry', 'automatedVerification', 'targetToken')) {
            if ([string]::IsNullOrWhiteSpace([string] $ability.$field)) {
                throw "ability $($ability.id) missing $field target=$($ability.targetEntry)"
            }
        }
        if (-not $ability.smokeCovered) {
            throw "ability $($ability.id) is not smoke covered target=$($ability.targetEntry)"
        }
        $abilityById[$ability.id] = $ability
    }

    foreach ($id in @(
            'dock.tool_output_viewer',
            'dock.shortcut_help',
            'system.mouse_wheel',
            'system.drag_select_text',
            'system.paste_text',
            'system.paste_image',
            'system.clear',
            'system.external_editor',
            'input.restore_interrupt',
            'slash.exit_aliases',
            'runtime.wire.file_change',
            'live.tool_lifecycle'
        )) {
        if (-not $abilityById.ContainsKey($id)) {
            throw "required ability missing: $id"
        }
    }

    Write-Host "ability matrix: total=$($matrix.totalCount) smoke=$($matrix.smokeCoveredCount) categories=$($matrix.categories.Count)"
    return $abilityById
}

function Assert-SmokeContractFailure {
    param(
        [string] $PointId,
        [string] $TargetEntry,
        [string] $Reason
    )

    throw "smoke point failed: id=$PointId target=$TargetEntry reason=$Reason"
}

function Assert-AgentTuiSmokeContract {
    param(
        [object] $Report,
        [hashtable] $AbilityById
    )

    $contract = $Report.smokeContract
    if ($null -eq $contract) {
        throw 'smokeContract missing from Agent TUI launch report'
    }
    if ($contract.totalPoints -lt 80) {
        throw "smokeContract totalPoints too small: $($contract.totalPoints)"
    }
    if ($contract.requiredPoints -lt 80) {
        throw "smokeContract requiredPoints too small: $($contract.requiredPoints)"
    }
    if ($null -eq $contract.points -or $contract.points.Count -ne $contract.totalPoints) {
        throw "smokeContract point count mismatch: points=$($contract.points.Count) total=$($contract.totalPoints)"
    }

    $requiredCategories = @(
        'header',
        'diff_cell',
        'viewport_history',
        'resume_snapshot',
        'shortcut',
        'dock',
        'approval',
        'cell_visual',
        'terminal',
        'unicode_width',
        'output_backpressure',
        'runtime_contract',
        'error_presentation',
        'sub_state_invariant',
        'frame',
        'editing_mouse',
        'prompt_cache'
    )
    $categoryNames = @($contract.categories | ForEach-Object { $_.category })
    foreach ($category in $requiredCategories) {
        if ($categoryNames -notcontains $category) {
            throw "smokeContract category missing: $category"
        }
    }

    $pointById = @{}
    foreach ($point in $contract.points) {
        $pointId = [string] $point.id
        $targetEntry = [string] $point.targetEntry
        if ([string]::IsNullOrWhiteSpace($pointId)) {
            Assert-SmokeContractFailure -PointId '<missing>' -TargetEntry $targetEntry -Reason 'id missing'
        }
        if ($pointById.ContainsKey($pointId)) {
            Assert-SmokeContractFailure -PointId $pointId -TargetEntry $targetEntry -Reason 'duplicate id'
        }
        foreach ($field in @('category', 'targetEntry', 'evidence', 'verification')) {
            if ([string]::IsNullOrWhiteSpace([string] $point.$field)) {
                Assert-SmokeContractFailure -PointId $pointId -TargetEntry $targetEntry -Reason "$field missing"
            }
        }
        if (-not [string]::IsNullOrWhiteSpace([string] $point.abilityId) -and -not $AbilityById.ContainsKey($point.abilityId)) {
            Assert-SmokeContractFailure -PointId $pointId -TargetEntry $targetEntry -Reason "abilityId not found: $($point.abilityId)"
        }
        $pointById[$pointId] = $point
    }

    $requiredPoints = @(
        'header.width.80',
        'header.width.120',
        'header.width.160',
        'diff.file_path',
        'diff.action_type',
        'diff.line_stats',
        'diff.hunk_line',
        'diff.key_line',
        'history.detached_streaming',
        'history.jump_bottom',
        'history.jump_threshold',
        'resume.header_snapshot',
        'resume.viewport_cells',
        'resume.composer_initial_state',
        'resume.read_only_snapshot',
        'shortcut.esc_control',
        'dock.shortcut_help',
        'dock.tool_output_enter',
        'dock.skills',
        'dock.mcp_catalog',
        'dock.alt_enter',
        'approval.enter_denies',
        'approval.y_once',
        'approval.a_confirm',
        'approval.esc_cancel',
        'resume.loading_cell',
        'resume.corrupt_inline_error',
        'resume.permission_inline_error',
        'cell.command.visual',
        'cell.diff.visual',
        'cell.message.visual',
        'cell.exec.visual',
        'cell.history.visual',
        'cell.hook.visual',
        'cell.mcp.visual',
        'cell.plan.visual',
        'cell.review.visual',
        'cell.status.visual',
        'cell.generation_status.visual',
        'cell.reasoning.visual',
        'cell.usage.visual',
        'cell.error.visual',
        'terminal.windows_terminal',
        'terminal.tmux',
        'terminal.screen',
        'terminal.ssh_slow_link',
        'unicode.cjk',
        'unicode.fullwidth_punctuation',
        'unicode.emoji',
        'unicode.zwj',
        'unicode.variation_selector',
        'unicode.skin_tone',
        'unicode.flag',
        'output.10mb',
        'output.100mb',
        'output.truncation_notice',
        'output.paged_artifact',
        'runtime.schema_mismatch',
        'runtime.capabilities',
        'runtime.steer_unsupported',
        'runtime.steer_rejected',
        'runtime.bounded_channel',
        'runtime.worker_panic',
        'error.provider_401',
        'error.provider_429',
        'error.provider_offline',
        'error.mcp_crash',
        'error.snapshot_corrupt',
        'error.snapshot_permission',
        'error.cell_payload_missing_field',
        'invariant.header_viewport_composer',
        'invariant.cell_selection_bounds',
        'invariant.dock_single_active',
        'invariant.approval_queue',
        'invariant.runtime_correlation',
        'invariant.cache_key_boundary',
        'frame.thousand_schedule',
        'frame.no_duplicate_stable',
        'frame.latency_metric',
        'editing.file_completion',
        'editing.drag_select',
        'prompt_cache.dock',
        'prompt_cache.scroll',
        'prompt_cache.selection',
        'prompt_cache.animation_tick'
    )
    foreach ($requiredPoint in $requiredPoints) {
        if (-not $pointById.ContainsKey($requiredPoint)) {
            Assert-SmokeContractFailure -PointId $requiredPoint -TargetEntry '<missing>' -Reason 'required smoke point missing'
        }
    }

    Write-Host "smoke contract: total=$($contract.totalPoints) required=$($contract.requiredPoints) categories=$($contract.categories.Count)"
}

Push-Location $repoRoot
try {
    Assert-PwshRuntime
    $report = Invoke-AgentTuiJsonSmoke
    if ($null -eq $report.telemetry) {
        throw 'telemetry snapshot missing from Agent TUI launch report'
    }

    $telemetry = $report.telemetry
    $terminalCompatibility = $report.terminalCompatibility
    if ($null -eq $terminalCompatibility) {
        throw 'terminal compatibility snapshot missing from Agent TUI launch report'
    }
    if ($null -eq $telemetry.tracing) {
        throw 'tracing snapshot missing from telemetry'
    }
    if ($telemetry.tracing.containsSensitiveFields) {
        throw 'tracing snapshot contains sensitive field names'
    }
    $abilityById = Assert-AgentTuiAbilityMatrix -Report $report
    Assert-AgentTuiSmokeContract -Report $report -AbilityById $abilityById

    $ttfr = if ($null -eq $telemetry.ttfrMs) { 'pending' } else { "$($telemetry.ttfrMs)ms" }
    $lastFrameLatency = if ($null -eq $telemetry.lastFrameLatencyMs) { 'pending' } else { "$($telemetry.lastFrameLatencyMs)ms" }
    $maxFrameLatency = if ($null -eq $telemetry.maxFrameLatencyMs) { 'pending' } else { "$($telemetry.maxFrameLatencyMs)ms" }
    $cacheHitRate = '{0:N2}%' -f ($telemetry.cacheHitRateBps / 100)
    $frameCoalescingRate = '{0:N2}%' -f ($telemetry.frameCoalescingRateBps / 100)
    $terminalModes = @(
        "osc8=$($terminalCompatibility.osc8Hyperlinks)"
        "osc52=$($terminalCompatibility.osc52Clipboard)"
        "mouse=$($terminalCompatibility.mouse)"
        "unicode=$($terminalCompatibility.unicodeWidth)"
        "resize=$($terminalCompatibility.resizeReflow)"
    ) -join ' '

    Write-Output "TTFR: $ttfr"
    Write-Output "frame latency: last=$lastFrameLatency max=$maxFrameLatency"
    Write-Output "frame coalesced: $($telemetry.frameCoalescedCount)"
    Write-Output "stable duplicate frames: $($telemetry.stableDuplicateFrameCount)"
    Write-Output "cache hit rate: $cacheHitRate"
    Write-Output "frame coalescing rate: $frameCoalescingRate"
    Write-Output "tracing spans: $($telemetry.tracing.spanCount)"
    Write-Output "terminal compatibility: profile=$($terminalCompatibility.profile) transport=$($terminalCompatibility.transport) $terminalModes hyperlink=$($terminalCompatibility.hyperlinkMode) copy=$($terminalCompatibility.copyMode) mouseMode=$($terminalCompatibility.mouseMode) replayRows=$($terminalCompatibility.replayMaxRows)"

    Assert-TerminalFixture -Name 'Windows Terminal' -Environment @{ WT_SESSION = 'smoke' } -ExpectedProfile 'windows_terminal' -ExpectedHyperlinkMode 'osc8' -ExpectedMouseMode 'keyboard_only'
    Assert-TerminalFixture -Name 'tmux' -Environment @{ TMUX = 'smoke' } -ExpectedProfile 'tmux' -ExpectedHyperlinkMode 'plain_text' -ExpectedMouseMode 'keyboard_only'
    Assert-TerminalFixture -Name 'screen' -Environment @{ STY = 'smoke' } -ExpectedProfile 'screen' -ExpectedHyperlinkMode 'plain_text' -ExpectedMouseMode 'keyboard_only'
    Assert-TerminalFixture -Name 'ssh slow link' -Environment @{ AXON_TUI_SSH_SLOW_LINK = '1'; TERM_PROGRAM = 'WezTerm' } -ExpectedProfile 'wezterm' -ExpectedHyperlinkMode 'osc8' -ExpectedMouseMode 'keyboard_only'
}
finally {
    Pop-Location
}
