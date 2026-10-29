[CmdletBinding()]
param([switch]$Capture,[string]$OutputDirectory,[switch]$Optimized)
$ErrorActionPreference='Stop'
$projectRoot=Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
try {
 $cargoArgs=@('run','--locked','--offline','-p','snow-draw-engine-gpui','--bin','snow-draw-demo')
 if($Optimized){$cargoArgs+=@('--profile','canvas-perf')}
 if($Capture){
  if([string]::IsNullOrWhiteSpace($OutputDirectory)){$OutputDirectory=Join-Path $projectRoot '../artifacts/snow-draw-gpui-native'}
  $cargoArgs+=@('--features','native-capture','--','--capture',[System.IO.Path]::GetFullPath($OutputDirectory))
 }
 & cargo @cargoArgs
 if($LASTEXITCODE -ne 0){throw "Canvas exited with code $LASTEXITCODE"}
} finally {Pop-Location}
