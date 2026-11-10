# 生成安装包：lenswin-<版本>-setup-x64.exe，需要 Inno Setup 6 的 ISCC.exe
# 用法：powershell -File packaging\build-installer.ps1 [-Iscc <ISCC.exe 路径>]
[CmdletBinding()]
param(
    [string]$Iscc,
    [string]$OutputDir
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $OutputDir) { $OutputDir = Join-Path $root 'dist' }

$cargoToml = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
if ($cargoToml -notmatch '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
    throw '无法从 Cargo.toml 读取版本号'
}
$version = $Matches[1]

# 依次尝试：参数、环境变量、PATH、Inno Setup 的默认安装位置
function Find-Iscc {
    param([string]$Explicit)
    $candidates = @($Explicit, $env:ISCC_PATH) | Where-Object { $_ }
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) { return $candidate }
    }
    $onPath = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $defaults = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
    )
    foreach ($candidate in $defaults) {
        if (Test-Path -LiteralPath $candidate) { return $candidate }
    }
    throw '找不到 ISCC.exe，请安装 Inno Setup 6，或用 -Iscc / 环境变量 ISCC_PATH 指定路径'
}
$isccPath = Find-Iscc -Explicit $Iscc

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
$exeVersion = (Get-Item -LiteralPath $exe).VersionInfo.FileVersion
if ($exeVersion -ne $version) {
    throw "exe 版本 $exeVersion 与 Cargo.toml 的 $version 不一致"
}

New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
& $isccPath `
    "/DAppVersion=$version" `
    "/DSourceExe=$exe" `
    "/DIconFile=$(Join-Path $root 'app\assets\lenswin.ico')" `
    "/DLicenseFile=$(Join-Path $root 'LICENSE')" `
    "/DOutputDir=$OutputDir" `
    (Join-Path $PSScriptRoot 'lenswin.iss')
if ($LASTEXITCODE -ne 0) { throw 'ISCC 编译失败' }
Write-Host "已生成 $(Join-Path $OutputDir "lenswin-$version-setup-x64.exe")"
