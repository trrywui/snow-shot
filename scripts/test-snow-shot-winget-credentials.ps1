#Requires -Version 7.0
# Read-only preflight. Never print or pass the submission token on a command line.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'snow-shot-winget.ps1')
$account = Get-SnowShotWingetSubmissionAccount
$headers = @{
    Authorization = "Bearer $env:WINGET_CREATE_GITHUB_TOKEN"
    Accept = 'application/vnd.github+json'
    'X-GitHub-Api-Version' = '2022-11-28'
}
$fork = Invoke-RestMethod -Uri "https://api.github.com/repos/$($account.login)/winget-pkgs" -Headers $headers
if (-not $fork.fork -or $fork.parent.full_name -cne 'microsoft/winget-pkgs' -or -not $fork.permissions.push) {
    throw 'The submission account needs a writable fork of microsoft/winget-pkgs named winget-pkgs.'
}
Write-Output "PASS: submission token authenticates as $($account.login), has repository and workflow scopes, and can write to $($fork.full_name). No submission performed."
