#Requires -Version 7.0
# Deterministic desktop fixtures: no real registry writes or process termination.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'snow-shot-winget-test-desktop.ps1')

function Require($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Expect-Failure([scriptblock]$Action, [string]$Pattern) {
    try { & $Action } catch {
        Require ($_.Exception.Message -match $Pattern) "Unexpected error: $_"
        return
    }
    throw "Expected failure matching $Pattern"
}
function New-FixtureProcess([int]$Id, [int]$SessionId, [bool]$HasExited = $false) {
    $process = [pscustomobject]@{ Id = $Id; SessionId = $SessionId; HasExited = $HasExited }
    $process | Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {
        param($Timeout)
        Require ($Timeout -eq 5000) 'First-logon process termination must be bounded.'
        return $script:terminationCompletes
    }
    return $process
}
function Test-Path {
    param($LiteralPath, $ErrorAction)
    Require ($LiteralPath -ceq $script:policy -and $ErrorAction -eq 'Stop') 'Unexpected registry lookup.'
    $script:operations.Add('lookup')
    return $script:policyExists
}
function New-Item {
    param($Path, [switch]$Force, $ErrorAction)
    Require ($Path -ceq $script:policy -and $Force -and $ErrorAction -eq 'Stop') 'Unexpected registry key creation.'
    $script:operations.Add('create')
}
function New-ItemProperty {
    param($LiteralPath, $Name, $Value, $PropertyType, [switch]$Force, $ErrorAction)
    Require ($LiteralPath -ceq $script:policy -and $Name -ceq 'DisablePrivacyExperience' -and
        $Value -eq 1 -and $PropertyType -ceq 'DWord' -and $Force -and $ErrorAction -eq 'Stop') `
        'Only the OOBE privacy experience policy may change, and errors must abort preparation.'
    $script:operations.Add('policy')
    if ($script:policyDenied) { throw 'Fixture registry access denied.' }
}
function Get-Process {
    param($Id, $Name, $ErrorAction)
    Require ($script:operations.Contains('policy')) 'Desktop processes must be queried after applying the policy.'
    if ($null -ne $Id) {
        Require ($Id -eq $PID) 'Use the test process session.'
        return [pscustomobject]@{ SessionId = 7 }
    }
    Require ($Name -in @('UserOOBEBroker', 'WWAHost')) 'Unrelated processes must never be queried.'
    $script:operations.Add($Name)
    return $script:processes[$Name]
}
function Stop-Process {
    param($InputObject, [switch]$Force, $ErrorAction)
    Require ($InputObject.SessionId -eq 7 -and $Force -and $ErrorAction -eq 'Stop') 'Only first-logon processes in this session may be stopped.'
    $script:operations.Add("stop-$($InputObject.Id)")
}

$runnerEnvironment = $env:RUNNER_ENVIRONMENT
$runnerOs = $env:RUNNER_OS
$script:policy = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\OOBE'
$script:operations = [Collections.Generic.List[string]]::new()
$script:policyExists = $false
$script:policyDenied = $false
$script:terminationCompletes = $true
$script:processes = @{}
try {
    foreach ($contents in @('', ' ', 'Window Dialog', "Window Dialog`r`n  `r`nWindow Dialog")) {
        Require (-not (Test-SnowShotWingetInteractiveDialog 'Window Dialog' $contents)) `
            'A shell launch container without prompt content must keep waiting for consent UI or process completion.'
    }
    Require (Test-SnowShotWingetInteractiveDialog 'Window Dialog' "Window Dialog`nCannot launch this app.`nClose") `
        'An interactive launch failure must still fail unattended installation.'
    Require (-not (Test-SnowShotWingetInteractiveDialog 'Open File - Security Warning' 'Run')) `
        'The security warning must be diagnosed separately from the generic shell container.'

    foreach ($hostCase in @(
        @{ Environment = ''; OS = 'Windows' },
        @{ Environment = 'self-hosted'; OS = 'Windows' },
        @{ Environment = 'github-hosted'; OS = 'Linux' }
    )) {
        $env:RUNNER_ENVIRONMENT = $hostCase.Environment
        $env:RUNNER_OS = $hostCase.OS
        Expect-Failure { Initialize-SnowShotWingetTestDesktop } 'requires a disposable GitHub-hosted Windows runner'
        Require ($script:operations.Count -eq 0) 'Host rejection must precede all registry and process operations.'
    }
    $env:RUNNER_ENVIRONMENT = 'github-hosted'
    $env:RUNNER_OS = 'Windows'
    $script:processes = @{
        UserOOBEBroker = @((New-FixtureProcess 10 7), (New-FixtureProcess 11 8))
        WWAHost = @((New-FixtureProcess 20 7), (New-FixtureProcess 21 8), (New-FixtureProcess 22 7 $true))
    }
    Initialize-SnowShotWingetTestDesktop
    Require (($script:operations -join ',') -ceq 'lookup,create,policy,UserOOBEBroker,stop-10,WWAHost,stop-20') `
        'Disable first-logon startup, then stop the broker and privacy host in the current session only.'

    $script:operations.Clear()
    $script:policyExists = $true
    $script:processes = @{ UserOOBEBroker = @(); WWAHost = @() }
    Initialize-SnowShotWingetTestDesktop
    Require (($script:operations -join ',') -ceq 'lookup,policy,UserOOBEBroker,WWAHost') `
        'An already prepared desktop must work without processes or registry key recreation.'

    $script:operations.Clear()
    $script:policyDenied = $true
    Expect-Failure { Initialize-SnowShotWingetTestDesktop } 'Fixture registry access denied'
    Require (($script:operations -join ',') -ceq 'lookup,policy') 'Policy failure must abort before process termination.'

    $script:policyDenied = $false
    $script:terminationCompletes = $false
    $script:processes.UserOOBEBroker = @((New-FixtureProcess 10 7))
    Expect-Failure { Initialize-SnowShotWingetTestDesktop } 'first-logon process UserOOBEBroker did not exit'
    Write-Output 'PASS: WinGet runner desktop preparation, host/session isolation, and failure propagation.'
} finally {
    $env:RUNNER_ENVIRONMENT = $runnerEnvironment
    $env:RUNNER_OS = $runnerOs
}
