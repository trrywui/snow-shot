#Requires -Version 7.2
[CmdletBinding()]
param(
    [ValidateSet('x64')][string]$Architecture = 'x64',
    [string]$IdentityPath = (Join-Path $PSScriptRoot '../snow_shot/packaging/msix/store-identity.json'),
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../artifacts/msix/packages'),
    [string]$ExpectedVersion,
    [hashtable]$OcrRuntimeArchives = @{},
    [ValidateRange(1, 256)][int]$Parallelism = 4,
    [switch]$SkipBuild,
    [string]$TestCertificateThumbprint
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'snow-shot-msix.ps1')
$identity = Read-SnowMsixIdentity $IdentityPath
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
$null = New-Item -ItemType Directory -Path $outputRoot -Force
$work = Join-Path $script:SnowMsixRepoRoot ('artifacts/msix/work-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $work -Force
$packages = @()
$evidence = @()
$parameters = @{ Architecture = $Architecture; DistributionChannel = 'store'; StageOnly = $true;
    SkipBuild = $SkipBuild; Parallelism = $Parallelism }
if ($OcrRuntimeArchives.ContainsKey($Architecture)) {
    $parameters.OcrRuntimeArchive = $OcrRuntimeArchives[$Architecture]
}
$stageResult = @(& (Join-Path $PSScriptRoot 'package-snow-shot.ps1') @parameters |
    ForEach-Object {
        if ($_.PSObject.Properties['StoreStage']) { $_ }
        else { Write-Host $_ }
    })
if ($stageResult.Count -ne 1) { throw 'The audited Store staging operation returned no unique stage.' }
$stageResult = $stageResult[0]
$audit = Assert-SnowMsixAuditedStage -Stage $stageResult.StoreStage -AuditPath $stageResult.AuditPath -Architecture $Architecture
if ($ExpectedVersion -and $audit.PackageVersion -cne $ExpectedVersion) {
    throw "Store package version '$($audit.PackageVersion)' does not match release version '$ExpectedVersion'. Rebuild the Store preset without -SkipBuild."
}
$version = ConvertTo-SnowMsixVersion $audit.PackageVersion
$packageStage = Join-Path $work $Architecture
$null = New-Item -ItemType Directory -Path $packageStage
Copy-Item -Path (Join-Path $stageResult.StoreStage '*') -Destination $packageStage -Recurse -Force
Write-SnowMsixManifest -Stage $packageStage -Identity $identity -Version $version -Architecture $Architecture
$package = Join-Path $outputRoot "snow-shot-$version-windows-$Architecture.msix"
New-SnowMsixPackage -Stage $packageStage -Destination $package -WorkDirectory $work
if ($TestCertificateThumbprint) {
    Set-SnowMsixTestSignature -Package $package -CertificateThumbprint $TestCertificateThumbprint -Publisher $identity.Publisher
}
$packages += $package
$evidence += [ordered]@{ Architecture = $Architecture; Version = $version;
    NativeValidationRequired = $audit.NativeValidationRequired; StageAudit = $stageResult.AuditPath;
    InstallFiles = (Get-SnowMsixInventory $packageStage) }
foreach ($package in $packages) {
    $hash = (Get-FileHash -LiteralPath $package -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($package))" | Set-Content -LiteralPath "$package.sha256" -Encoding ascii
}
[ordered]@{ SchemaVersion = 1; Channel = 'store'; Identity = $identity;
    SignedForTesting = [bool]$TestCertificateThumbprint; Architectures = $evidence;
    Packages = @($packages | ForEach-Object { [ordered]@{ Path = $_;
        Bytes = (Get-Item -LiteralPath $_).Length; Sha256 = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant() } }) } |
    ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $outputRoot 'store-packages.manifest.json') -Encoding utf8NoBOM
Write-Output "Store package evidence: $(Join-Path $outputRoot 'store-packages.manifest.json')"
$packages | Write-Output
