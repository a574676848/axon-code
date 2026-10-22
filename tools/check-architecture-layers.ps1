param(
    [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

$ErrorActionPreference = "Stop"

function Fail([string]$Message) {
    Write-Error $Message
    exit 1
}

function Get-NormalizedWorkspacePath([string]$Root, [string]$Path) {
    $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    $pathFull = [System.IO.Path]::GetFullPath($Path)
    $rootUri = New-Object System.Uri(($rootFull + [System.IO.Path]::DirectorySeparatorChar))
    $pathUri = New-Object System.Uri($pathFull)
    return [System.Uri]::UnescapeDataString($rootUri.MakeRelativeUri($pathUri).ToString()).Replace("\", "/")
}

function Get-RustFiles([string]$Root, [string]$RelativeDir) {
    $dir = Join-Path $Root $RelativeDir
    if (-not (Test-Path -LiteralPath $dir)) {
        return @()
    }
    return @(Get-ChildItem -Path $dir -Recurse -File -Filter *.rs)
}

function Find-RuleHits([string]$Root, [object[]]$Rules) {
    $hits = @()
    foreach ($rule in $Rules) {
        foreach ($file in (Get-RustFiles $Root $rule.RelativeDir)) {
            $lineNumber = 0
            foreach ($line in [System.IO.File]::ReadLines($file.FullName, [System.Text.Encoding]::UTF8)) {
                $lineNumber += 1
                foreach ($pattern in $rule.Patterns) {
                    if ($line -match $pattern) {
                        $relativePath = Get-NormalizedWorkspacePath $Root $file.FullName
                        $hits += "${relativePath}:${lineNumber}: $($rule.Message): $($line.Trim())"
                    }
                }
            }
        }
    }
    return $hits
}

function Find-ScopedRuleHits([string]$Root, [object[]]$Rules) {
    $hits = @()
    foreach ($rule in $Rules) {
        foreach ($file in (Get-RustFiles $Root $rule.RelativeDir)) {
            $relativePath = Get-NormalizedWorkspacePath $Root $file.FullName
            if ($rule.AllowedRelativePaths -contains $relativePath) {
                continue
            }
            $lineNumber = 0
            foreach ($line in [System.IO.File]::ReadLines($file.FullName, [System.Text.Encoding]::UTF8)) {
                $lineNumber += 1
                foreach ($pattern in $rule.Patterns) {
                    if ($line -match $pattern) {
                        $hits += "${relativePath}:${lineNumber}: $($rule.Message): $($line.Trim())"
                    }
                }
            }
        }
    }
    return $hits
}

$rules = @(
    @{
        RelativeDir = "crates/axon-tui-runtime/src/surface";
        Message = "surface must not depend on app shell, widgets, loop host, CLI, or terminal UI";
        Patterns = @(
            "crate::app_shell\b",
            "crate::widgets\b",
            "crate::r#loop::agent_tui\b",
            "\baxon_cli::",
            "\bratatui::",
            "\bcrossterm::"
        )
    },
    @{
        RelativeDir = "crates/axon-tui-runtime/src/widgets";
        Message = "widgets must remain pure renderers and must not own workflow/runtime state";
        Patterns = @(
            "crate::ui_workflow\b",
            "\bRuntimeSubmissionDriver\b",
            "\bAgentTuiRuntimeAdapter\b",
            "\bAgentTuiSubmit\b",
            "&mut\s+CellStore\b"
        )
    },
    @{
        RelativeDir = "crates/axon-tui-runtime/src/ui_workflow";
        Message = "ui workflow must not execute runtime submit or depend on terminal widgets";
        Patterns = @(
            "\bRuntimeSubmissionDriver\b",
            "\bAgentTuiRuntimeAdapter\b",
            "\bAgentTuiSubmit\b",
            "\bRuntimeSubmit\s*\(\s*AgentTuiSubmit\s*\)",
            "\bratatui::",
            "\bcrossterm::",
            "\.submit\s*\("
        )
    },
    @{
        RelativeDir = "crates/axon-tui-runtime/src/app_shell";
        Message = "app shell must not depend on CLI, provider/session internals, or runtime submit driver";
        Patterns = @(
            "\baxon_cli::",
            "\baxon_session::",
            "\bRuntimeSubmissionDriver\b",
            "\bAgentTuiRuntimeAdapter\b"
        )
    },
    @{
        RelativeDir = "crates/axon-tui-runtime/src/surface/payloads";
        Message = "protocol DTO must not depend on terminal UI, provider/session, widgets, or render abstractions";
        Patterns = @(
            "\bratatui::",
            "\bcrossterm::",
            "\baxon_session::",
            "\baxon_agent_runtime::",
            "crate::widgets\b",
            "\bRenderCell\b",
            "\bRenderPolicy\b",
            "\bCellRegistry\b",
            "\bWidget\b",
            "\bSpan\b",
            "\bLine\b",
            "\bStyle\b",
            "\bParagraph\b"
        )
    },
    @{
        RelativeDir = "crates/axon-cli/src/run_commands/interactive_tui";
        Message = "CLI interactive TUI host must not reference terminal widget internals";
        Patterns = @(
            "\bratatui::",
            "\bcrossterm::",
            "\baxon_tui_runtime::widgets\b",
            "\bcrate::widgets\b",
            "\bwidgets::",
            "\bratatui::widgets::(Widget|Paragraph)\b",
            "\bratatui::text::(Span|Line)\b",
            "\bratatui::style::Style\b",
            "\buse\s+ratatui::.*\b(Widget|Paragraph|Span|Line|Style)\b"
        )
    }
)

$hits = Find-RuleHits $WorkspaceRoot $rules
if ($hits.Count -gt 0) {
    Fail ("TUI architecture layer violation detected:" + [Environment]::NewLine + ($hits -join [Environment]::NewLine))
}

$singleOwnerRules = @(
    @{
        RelativeDir = "crates/axon-tui-runtime/src";
        Message = "spinner frames and spinner interval must be owned by surface/animation.rs";
        AllowedRelativePaths = @(
            "crates/axon-tui-runtime/src/surface/animation.rs"
        );
        Patterns = @(
            "\bconst\s+SPINNER_FRAMES\b",
            "\bSPINNER_INTERVAL\s*:\s*Duration\b"
        )
    },
    @{
        RelativeDir = "crates/axon-tui-runtime/src";
        Message = "target-state palette tokens must be owned by app_shell/style_tokens.rs";
        AllowedRelativePaths = @(
            "crates/axon-tui-runtime/src/app_shell/style_tokens.rs"
        );
        Patterns = @(
            "Color::Rgb\(191,\s*203,\s*219\)",
            "Color::Rgb\(165,\s*180,\s*202\)",
            "Color::Rgb\(119,\s*130,\s*150\)",
            "Color::Rgb\(83,\s*205,\s*137\)",
            "Color::Rgb\(147,\s*167,\s*194\)",
            "Color::Rgb\(103,\s*197,\s*226\)",
            "Color::Rgb\(229,\s*194,\s*103\)",
            "Color::Rgb\(192,\s*162,\s*255\)",
            "Color::Rgb\(255,\s*111,\s*111\)"
        )
    }
)

$singleOwnerHits = Find-ScopedRuleHits $WorkspaceRoot $singleOwnerRules
if ($singleOwnerHits.Count -gt 0) {
    Fail ("TUI single-owner rule violation detected:" + [Environment]::NewLine + ($singleOwnerHits -join [Environment]::NewLine))
}

$abstractionLeakRules = @(
    @{
        RelativeDir = "crates/axon-tui-runtime/src/surface";
        Message = "runtime selectability belongs to RenderPolicy, not RenderCell or payload";
        AllowedRelativePaths = @(
            "crates/axon-tui-runtime/src/surface/render_policy.rs"
        );
        Patterns = @(
            "\bis_runtime_selectable\b"
        )
    },
    @{
        RelativeDir = "crates/axon-tui-runtime/src/surface/payloads";
        Message = "payload kind must use CellPayloadKind and registry, not raw static strings";
        AllowedRelativePaths = @();
        Patterns = @(
            "kind\s*:\s*&'static\s+str\b",
            "fn\s+kind\s*\([^)]*\)\s*->\s*&'static\s+str\b"
        )
    }
)

$abstractionLeakHits = Find-ScopedRuleHits $WorkspaceRoot $abstractionLeakRules
if ($abstractionLeakHits.Count -gt 0) {
    Fail ("TUI abstraction leak violation detected:" + [Environment]::NewLine + ($abstractionLeakHits -join [Environment]::NewLine))
}

Write-Output "TUI architecture layer verification OK"
