[CmdletBinding()]
param(
    [string]$OutputFile,
    [ValidateNotNullOrEmpty()][string]$Workload = 'all',
    [ValidateRange(1, 100)][int]$Samples = 5,
    [ValidateSet('cpu', 'gpu', 'donors', 'pipeline')][string]$Backend = 'cpu'
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$repoRoot = Split-Path -Parent $projectRoot
. (Join-Path $repoRoot 'scripts/snow-build-environment.ps1')
$null = Set-SnowBuildEnvironment -Preset windows-msvc-performance
if ([string]::IsNullOrWhiteSpace($OutputFile)) {
    $OutputFile = Join-Path $repoRoot 'artifacts/snow-draw-smart-erase.json'
}
$OutputFile = [System.IO.Path]::GetFullPath($OutputFile)
$null = New-Item -ItemType Directory -Force -Path (Split-Path -Parent $OutputFile)
Push-Location -LiteralPath $projectRoot
try {
    # This standalone package uses its own Release-derived profile and pinned Rust.
    & cargo bench --locked --offline -p snow-draw-engine-gpui --bench smart_erase `
        --profile canvas-perf -- $OutputFile $Workload $Samples $Backend.ToLowerInvariant()
    if ($LASTEXITCODE -ne 0) { throw 'Smart Erase benchmark failed' }
} finally {
    Pop-Location
}
