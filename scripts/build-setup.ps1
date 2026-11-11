# Builds the setup.exe of one Jay app around its NSIS installer.
# Example: build-setup.ps1 -Payload Jay-S_0.1.1_x64-setup.exe -Name Jay-S -Version 0.1.1 -Exe jay-s.exe -Icon icon.ico -Mark mark.png
param(
    [Parameter(Mandatory = $true)][string]$Payload,
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Icon,
    [Parameter(Mandatory = $true)][string]$Mark,
    [string]$OutDir = (Get-Location).Path
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

foreach ($file in $Payload, $Icon, $Mark) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "File not found: $file" }
}
$payloadPath = (Resolve-Path -LiteralPath $Payload).Path
$iconPath = (Resolve-Path -LiteralPath $Icon).Path
$markPath = (Resolve-Path -LiteralPath $Mark).Path
if (-not (Test-Path -LiteralPath $OutDir -PathType Container)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
$outDirPath = (Resolve-Path -LiteralPath $OutDir).Path

$names = 'JAYI_PAYLOAD', 'JAYI_NAME', 'JAYI_VERSION', 'JAYI_EXE', 'JAYI_MARK', 'TAURI_CONFIG'
Push-Location $root
try {
    $env:JAYI_PAYLOAD = $payloadPath
    $env:JAYI_NAME = $Name
    $env:JAYI_VERSION = $Version
    $env:JAYI_EXE = $Exe
    $env:JAYI_MARK = $markPath
    $config = @{ productName = $Name; version = $Version; bundle = @{ icon = @($iconPath) } }
    $env:TAURI_CONFIG = $config | ConvertTo-Json -Depth 5 -Compress

    cargo build --release -p jay-i-setup --bin jay-i-setup
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }

    $meta = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed ($LASTEXITCODE)" }
    $built = Join-Path $meta.target_directory 'release\jay-i-setup.exe'
    $target = Join-Path $outDirPath ("{0}_{1}_x64-setup.exe" -f $Name, $Version)
    Copy-Item -LiteralPath $built -Destination $target -Force
    $item = Get-Item -LiteralPath $target
    Write-Host ("{0} ({1} bytes)" -f $item.FullName, $item.Length)
}
finally {
    foreach ($n in $names) { Remove-Item -Path "Env:$n" -ErrorAction SilentlyContinue }
    Pop-Location
}
