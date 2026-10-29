[CmdletBinding()]
param(
    [string]$OutputFile,
    [ValidateSet('idle', 'smart-erase', 'smart-erase-polling', 'view-texture', 'view-layered', 'view-affine', 'view-periodic', 'view-texture-compare', 'view-layered-compare')][string]$Load = 'idle',
    [ValidateRange(1, 256)][int]$MaskWidth = 128,
    [ValidateRange(1, 192)][int]$MaskHeight = 96
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$repoRoot = Split-Path -Parent $projectRoot
. (Join-Path $repoRoot 'scripts/snow-build-environment.ps1')
$null = Set-SnowBuildEnvironment -Preset windows-msvc-performance
if ([string]::IsNullOrWhiteSpace($OutputFile)) {
    $OutputFile = Join-Path $repoRoot "artifacts/snow-draw-native-input-$Load.json"
}
$OutputFile = [System.IO.Path]::GetFullPath($OutputFile)
$null = New-Item -ItemType Directory -Force -Path (Split-Path -Parent $OutputFile)
Push-Location -LiteralPath $projectRoot
try {
    & cargo bench --locked --offline -p snow-draw-engine-gpui --bench native_input `
        --profile canvas-perf -- $OutputFile $Load.ToLowerInvariant() $MaskWidth $MaskHeight
    if ($LASTEXITCODE -ne 0) { throw 'Native input benchmark failed' }
} finally {
    Pop-Location
}
