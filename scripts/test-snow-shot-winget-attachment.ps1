#Requires -Version 7.0
# Registry fixtures only; never changes the developer's attachment policy.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'snow-shot-winget-test-desktop.ps1')
function Require($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Test-Path {
    param($LiteralPath, $ErrorAction)
    Require ($LiteralPath -ceq $script:path -and $ErrorAction -eq 'Stop') 'Unexpected policy path.'
    $script:accesses++
    return $script:exists
}
function Get-ItemProperty {
    param($LiteralPath, $ErrorAction)
    Require ($LiteralPath -ceq $script:path -and $ErrorAction -eq 'Stop') 'Unexpected policy read.'
    if ($script:present) { return [pscustomobject]@{ LowRiskFileTypes = $script:value } }
    return [pscustomobject]@{}
}
function Get-Item {
    param($LiteralPath, $ErrorAction)
    Require ($LiteralPath -ceq $script:path -and $ErrorAction -eq 'Stop') 'Unexpected registry kind read.'
    $key = [pscustomobject]@{}
    $key | Add-Member ScriptMethod GetValueKind { param($Name)
        Require ($Name -ceq 'LowRiskFileTypes') 'Unexpected registry value.'
        return [Microsoft.Win32.RegistryValueKind]::$script:kind
    }
    return $key
}
function New-Item {
    param($Path, [switch]$Force, $ErrorAction)
    Require ($Path -ceq $script:path -and $Force -and $ErrorAction -eq 'Stop') 'Unexpected policy key write.'
    $script:exists = $true
}
function New-ItemProperty {
    param($LiteralPath, $Name, $Value, $PropertyType, [switch]$Force, $ErrorAction)
    Require ($LiteralPath -ceq $script:path -and $Name -ceq 'LowRiskFileTypes' -and
        $Force -and $ErrorAction -eq 'Stop') 'Only the attachment consent list may change.'
    $script:present = $true
    $script:value = $Value
    $script:kind = $PropertyType
    if ($script:failWrite) { $script:failWrite = $false; throw 'Fixture policy write failed.' }
}
function Remove-ItemProperty {
    param($LiteralPath, $Name, $ErrorAction)
    Require ($LiteralPath -ceq $script:path -and $Name -ceq 'LowRiskFileTypes') 'Unexpected policy removal.'
    $script:present = $false
}
$runnerEnvironment = $env:RUNNER_ENVIRONMENT
$runnerOs = $env:RUNNER_OS
$script:path = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Policies\Associations'
$script:exists = $false
$script:present = $false
$script:value = ''
$script:kind = 'String'
$script:accesses = 0
$script:failWrite = $false
try {
    foreach ($hostCase in @(
        @{ Environment = ''; OS = 'Windows' },
        @{ Environment = 'self-hosted'; OS = 'Windows' },
        @{ Environment = 'github-hosted'; OS = 'Linux' }
    )) {
        $env:RUNNER_ENVIRONMENT = $hostCase.Environment
        $env:RUNNER_OS = $hostCase.OS
        $rejected = $false
        try { Enable-SnowShotWingetTestAttachmentConsent } catch {
            $rejected = $_.Exception.Message.Contains('requires a disposable GitHub-hosted Windows runner')
        }
        Require ($rejected -and $script:accesses -eq 0) 'Host rejection must precede registry access.'
    }
    $env:RUNNER_ENVIRONMENT = 'github-hosted'
    $env:RUNNER_OS = 'Windows'
    foreach ($existing in @($null, '', '.txt;.pdf', '.txt;.EXE')) {
        $script:present = $null -ne $existing
        $script:exists = $script:present
        $script:value = $existing
        $script:kind = 'ExpandString'
        $saved = Enable-SnowShotWingetTestAttachmentConsent
        Require (($script:value -split ';') -icontains '.exe') 'Unsigned executable fixtures require explicit launch consent.'
        Require (@($script:value -split ';' | Where-Object { $_ -ieq '.exe' }).Count -eq 1) 'Do not duplicate existing executable consent.'
        if ($existing -like '.txt*') { Require ($script:value.StartsWith($existing)) 'Preserve existing file types.' }
        Restore-SnowShotWingetTestAttachmentConsent $saved
        Require ($script:present -eq ($null -ne $existing)) 'Restore original policy presence.'
        if ($script:present) {
            Require ($script:value -ceq $existing -and $script:kind -ceq 'ExpandString') 'Restore the exact policy value and registry kind.'
        }
    }
    $script:present = $true
    $script:value = '.pdf'
    $script:failWrite = $true
    $failed = $false
    try { Enable-SnowShotWingetTestAttachmentConsent } catch {
        $failed = $_.Exception.Message -eq 'Fixture policy write failed.'
    }
    Require ($failed -and $script:value -ceq '.pdf') 'A partially applied policy must be restored before propagating failure.'
    Write-Output 'PASS: disposable fixture attachment consent, host isolation, exact restoration, and partial failure cleanup.'
} finally {
    $env:RUNNER_ENVIRONMENT = $runnerEnvironment
    $env:RUNNER_OS = $runnerOs
}
