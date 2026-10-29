#Requires -Version 7.0
# Desktop preparation for real installation tests, never for normal client provisioning.
function Enable-SnowShotWingetTestAttachmentConsent {
    if ($env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_OS -ne 'Windows') {
        throw 'Attachment consent requires a disposable GitHub-hosted Windows runner.'
    }
    # SmartScreen and Attachment Manager are separate consent mechanisms. This
    # explicit unsigned-fixture test opt-in must cover ShellExecute's file warning
    # without relying on desktop focus or synthetic button clicks.
    # https://learn.microsoft.com/windows/client-management/mdm/policy-csp-admx-attachmentmanager#am_setlowriskinclusion
    $path = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Policies\Associations'
    $property = $null
    $kind = 'String'
    if (Test-Path -LiteralPath $path -ErrorAction Stop) {
        $existing = Get-ItemProperty -LiteralPath $path -ErrorAction Stop
        $property = $existing.PSObject.Properties['LowRiskFileTypes']
        if ($property) { $kind = (Get-Item -LiteralPath $path -ErrorAction Stop).GetValueKind('LowRiskFileTypes').ToString() }
    }
    $saved = @{ Path = $path; Present = ($null -ne $property); Kind = $kind
        Value = if ($property) { $property.Value } else { '' } }
    $extensions = [string]$saved.Value
    if (@($extensions -split ';' | ForEach-Object { $_.Trim() }) -icontains '.exe') { return $saved }
    try {
        $null = New-Item -Path $path -Force -ErrorAction Stop
        $value = if ($extensions) { "$extensions;.exe" } else { '.exe' }
        $null = New-ItemProperty -LiteralPath $path -Name LowRiskFileTypes -Value $value `
            -PropertyType String -Force -ErrorAction Stop
    } catch {
        Restore-SnowShotWingetTestAttachmentConsent $saved
        throw
    }
    return $saved
}

function Restore-SnowShotWingetTestAttachmentConsent($Saved) {
    if ($Saved.Present) {
        $null = New-ItemProperty -LiteralPath $Saved.Path -Name LowRiskFileTypes -Value $Saved.Value `
            -PropertyType $Saved.Kind -Force -ErrorAction Stop
    } elseif (Test-Path -LiteralPath $Saved.Path -ErrorAction Stop) {
        $current = Get-ItemProperty -LiteralPath $Saved.Path -ErrorAction Stop
        if ($current.PSObject.Properties['LowRiskFileTypes']) {
            Remove-ItemProperty -LiteralPath $Saved.Path -Name LowRiskFileTypes -ErrorAction Stop
        }
    }
}

function Test-SnowShotWingetInteractiveDialog([string]$Title, [string]$Contents) {
    # ShellExecute can expose a window with only this generic caption while its
    # launch UI is initializing. Keep waiting for exit or the normal
    # command deadline until there is actual dialog content to diagnose.
    if ($Title -cne 'Window Dialog') { return $false }
    $contentLines = @($Contents -split '\r?\n' |
        Where-Object { $_.Trim() -and $_.Trim() -cne $Title })
    return $contentLines.Count -gt 0
}

function Initialize-SnowShotWingetTestDesktop {
    if ($env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_OS -ne 'Windows') {
        throw 'Desktop preparation requires a disposable GitHub-hosted Windows runner.'
    }

    # Windows ARM64 images can still be in first logon when the job starts, hiding
    # installer launch warnings behind the full-screen privacy experience.
    # https://github.com/actions/runner-images/issues/14069
    # Apply the documented policy before closing the already running experience.
    $policy = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\OOBE'
    if (-not (Test-Path -LiteralPath $policy -ErrorAction Stop)) {
        $null = New-Item -Path $policy -Force -ErrorAction Stop
    }
    $null = New-ItemProperty -LiteralPath $policy -Name DisablePrivacyExperience `
        -Value 1 -PropertyType DWord -Force -ErrorAction Stop

    $sessionId = (Get-Process -Id $PID -ErrorAction Stop).SessionId
    # Stop the broker first so it cannot relaunch its WWAHost privacy screen.
    foreach ($name in @('UserOOBEBroker', 'WWAHost')) {
        $processes = @(Get-Process -Name $name -ErrorAction SilentlyContinue |
            Where-Object { $_.SessionId -eq $sessionId })
        foreach ($process in $processes) {
            if ($process.HasExited) { continue }
            Write-Host "Closing runner first-logon process $name ($($process.Id))."
            Stop-Process -InputObject $process -Force -ErrorAction Stop
            if (-not $process.WaitForExit(5000)) {
                throw "Runner first-logon process $name did not exit."
            }
        }
    }
}
