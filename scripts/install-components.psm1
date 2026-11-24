<#
.SYNOPSIS
Axon 源码安装的组件集合解析模块。

.DESCRIPTION
把安装脚本的 -Components 取值解析成 InstallBackend 与 InstallDesktop 两个布尔字段，供后续每一步判断。
允许取值：backend、desktop、all。多值可用逗号分隔，也可重复传参；缺省或空值等价 all。
本模块只做纯计算，不读写文件、不启动进程，因此可以在不安装任何东西的前提下被自检脚本直接调用。
#>

Set-StrictMode -Version Latest

function Resolve-AxonComponents {
    <#
    .SYNOPSIS
    把组件集合解析为两个布尔字段。

    .PARAMETER Components
    组件名集合。允许 backend、desktop、all；可逗号分隔多值，可重复传参；缺省或空值等价 all。
    出现未知组件名时抛出异常，异常文本列出可用取值。

    .OUTPUTS
    含 InstallBackend 与 InstallDesktop 两个布尔字段的对象。
    #>
    [CmdletBinding()]
    param(
        [AllowNull()]
        [AllowEmptyCollection()]
        [string[]]$Components
    )

    $names = [System.Collections.Generic.List[string]]::new()
    foreach ($raw in @($Components)) {
        if ($null -eq $raw) { continue }
        foreach ($part in ($raw -split ',')) {
            $name = $part.Trim()
            if ($name) { $names.Add($name.ToLowerInvariant()) }
        }
    }
    if ($names.Count -eq 0) { $names.Add("all") }

    $installBackend = $false
    $installDesktop = $false
    foreach ($name in $names) {
        switch ($name) {
            "all" { $installBackend = $true; $installDesktop = $true }
            "backend" { $installBackend = $true }
            "desktop" { $installDesktop = $true }
            default { throw "未知组件名：$name。可用取值为 backend、desktop、all。" }
        }
    }

    [pscustomobject]@{
        InstallBackend = $installBackend
        InstallDesktop = $installDesktop
    }
}

Export-ModuleMember -Function Resolve-AxonComponents
