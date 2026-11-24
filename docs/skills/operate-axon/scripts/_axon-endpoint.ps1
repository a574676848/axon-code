# Axon 运行态端点解析助手：监听地址与 API token 的唯一解析入口。
# 被 scripts 目录下的调用脚本点源引用，不单独执行。不打印 token 原文。
Set-StrictMode -Off

function Invoke-AxonText([string[]]$Arguments) {
    $command = Get-Command axon -CommandType Application -ErrorAction SilentlyContinue
    if ($null -eq $command) { return '' }
    try { return (& $command.Source @Arguments 2>$null | Out-String).Trim() } catch { return '' }
}

function Get-AxonConfigValue([string]$Key) {
    $raw = Invoke-AxonText @('config', 'get', $Key)
    if (-not $raw) { return '' }
    $separator = $raw.IndexOf('=')
    if ($separator -lt 0) { return $raw }
    return $raw.Substring($separator + 1).Trim()
}

function Get-AxonEnvFilePath {
    $path = Invoke-AxonText @('config', 'env-path')
    if ($path) { return $path }
    $config_path = Invoke-AxonText @('config', 'path')
    if ($config_path) { return (Join-Path (Split-Path -Parent $config_path) '.env') }
    return ''
}

function Convert-BindToHostPort([string]$Bind) {
    if (-not $Bind) { return '' }
    $host_part, $port_part = $Bind -split ':', 2
    $host_part = $host_part.Trim('[', ']')
    if (-not $host_part -or $host_part -eq '0.0.0.0' -or $host_part -eq '::') { $host_part = '127.0.0.1' }
    if (-not $port_part) { return '' }
    return @($host_part, $port_part)
}

# 解析面基址：gateway（HTTP）、websocket（WS）、serve（headless server）。
# BaseUrl 显式给出时原样采用；否则按面的配置键推导，全部取值来自运行态配置。
function Resolve-EndpointBaseUrl([string]$Surface, [string]$BaseUrl) {
    if ($BaseUrl) { return $BaseUrl.TrimEnd('/') }
    switch ($Surface) {
        'serve' { return 'http://127.0.0.1:3000' }
        'websocket' {
            $parts = Convert-BindToHostPort (Get-AxonConfigValue 'remote.gateway.websocket_bind')
            if (-not $parts) { return '' }
            return ("ws://{0}:{1}" -f $parts[0], $parts[1])
        }
        default {
            $parts = Convert-BindToHostPort (Get-AxonConfigValue 'remote.gateway.http_bind')
            if (-not $parts) { return '' }
            return ("http://{0}:{1}" -f $parts[0], $parts[1])
        }
    }
}

# 解析 API token：显式参数、token 文件、进程环境、配置相邻 env 文件，按序取第一个命中。
function Resolve-EndpointToken([string]$Token, [string]$TokenFile) {
    if ($Token) { return $Token.Trim() }
    if ($TokenFile) {
        if (-not (Test-Path -LiteralPath $TokenFile -PathType Leaf)) { return '' }
        return ([IO.File]::ReadAllText($TokenFile)).Trim()
    }
    $env_name = Get-AxonConfigValue 'remote.gateway.api_token_env'
    if (-not $env_name) { return '' }
    $value = [Environment]::GetEnvironmentVariable($env_name)
    if ($value) { return $value.Trim() }
    $env_file = Get-AxonEnvFilePath
    if ($env_file -and (Test-Path -LiteralPath $env_file -PathType Leaf)) {
        foreach ($line in [IO.File]::ReadAllLines($env_file)) {
            $trimmed = $line.Trim()
            if (-not $trimmed -or $trimmed.StartsWith('#')) { continue }
            $separator = $trimmed.IndexOf('=')
            if ($separator -lt 0) { continue }
            if ($trimmed.Substring(0, $separator).Trim() -ne $env_name) { continue }
            return $trimmed.Substring($separator + 1).Trim().Trim('"', "'")
        }
    }
    return ''
}
