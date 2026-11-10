# 生成便携版 zip：lenswin-<版本>-portable-x64.zip
# 用法：powershell -File packaging\build-portable.ps1
[CmdletBinding()]
param(
    [string]$OutputDir
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $OutputDir) { $OutputDir = Join-Path $root 'dist' }

# 版本号唯一来源：根 Cargo.toml 的 [workspace.package] version
$cargoToml = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
if ($cargoToml -notmatch '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
    throw '无法从 Cargo.toml 读取版本号'
}
$version = $Matches[1]

Push-Location $root
try {
    cargo build --release -p lenswin
    if ($LASTEXITCODE -ne 0) { throw 'cargo build 失败' }
}
finally {
    Pop-Location
}

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$exe = Join-Path $targetDir 'release\lenswin.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw "找不到 $exe" }

# exe 内嵌的版本必须与 Cargo.toml 一致
$exeVersion = (Get-Item -LiteralPath $exe).VersionInfo.FileVersion
if ($exeVersion -ne $version) {
    throw "exe 版本 $exeVersion 与 Cargo.toml 的 $version 不一致"
}

$stage = Join-Path $OutputDir 'portable\lenswin'
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'lenswin.exe')
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'README.txt') -Destination $stage
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination $stage
# 空文件即可，程序只检查是否存在
New-Item -ItemType File -Path (Join-Path $stage 'portable') -Force | Out-Null

$zip = Join-Path $OutputDir "lenswin-$version-portable-x64.zip"
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Host "已生成 $zip"
