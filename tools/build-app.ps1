# 发包（桌面应用）：构建 hawk 桌面应用的分发包并归置到输出目录（默认仓库根目录的 out/，
# Windows 为免安装 hawk-windows-x64.zip）。
# 浏览器插件是独立命令 tools/build-extension.ps1；统一入口 tools/build.ps1 --platform app。
#
# 用法: ./tools/build-app.ps1 [-Path <输出目录>] [--unpacked]
#   --unpacked：只产出未打包目录（dist/win-unpacked），跳过 zip 压缩——install.ps1 的复用入口
# 前置: 最新 Node.js 与 Rust 工具链（https://rustup.rs/）
# 压缩级别（默认 5）: ELECTRON_BUILDER_COMPRESSION_LEVEL=9 ./tools/build-app.ps1   # 9=最小体积，3=最快

$ErrorActionPreference = 'Stop'

$Path = ''
$Unpacked = $false
$argsList = @($args)
$i = 0
while ($i -lt $argsList.Count) {
    $a = [string]$argsList[$i]
    if ($a -match '^(-Path|--path)$') {
        if ($i + 1 -ge $argsList.Count) { throw "$a 需要目录参数" }
        $Path = [string]$argsList[$i + 1]; $i += 2
    } elseif ($a -match '^--?path=(.+)$') {
        $Path = $Matches[1]; $i++
    } elseif ($a -match '^(-Unpacked|--unpacked)$') {
        $Unpacked = $true; $i++
    } else {
        throw "未知参数: $a（用法: ./tools/build-app.ps1 [-Path <输出目录>] [--unpacked]）"
    }
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
$AppDir = Join-Path $RepoRoot 'hawk-app'
$OutDir = if ($Path) { $Path } else { Join-Path $RepoRoot 'out' }

foreach ($tool in @('node', 'npm', 'cargo')) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "未找到 $tool，请先安装最新的 Node.js 与 Rust 工具链（https://rustup.rs/）"
    }
}

# Electron 包（npm install）与 electron-builder（pack）的二进制下载默认走 npmmirror（国内网络；
# 用户已设置同名环境变量时尊重用户配置）
if (-not $env:ELECTRON_MIRROR) { $env:ELECTRON_MIRROR = 'https://npmmirror.com/mirrors/electron/' }
if (-not $env:ELECTRON_BUILDER_BINARIES_MIRROR) { $env:ELECTRON_BUILDER_BINARIES_MIRROR = 'https://npmmirror.com/mirrors/electron-builder-binaries/' }

Push-Location $AppDir
try {
    if (-not (Test-Path 'node_modules')) {
        npm install
    }
    if ($Unpacked) {
        npm run pack:dir # install 只需要未打包目录，跳过 zip 压缩
    } else {
        npm run pack
    }
} finally {
    Pop-Location
}

if ($Unpacked) {
    Write-Host "未打包产物: $AppDir\dist\win-unpacked"
    return
}

$package = Join-Path $AppDir 'dist\hawk-windows-x64.zip'
if (-not (Test-Path $package)) {
    throw "打包产物不存在: $package（electron-builder 未产出 hawk-windows-x64.zip）"
}
New-Item -ItemType Directory -Force $OutDir | Out-Null
Copy-Item $package (Join-Path $OutDir 'hawk-windows-x64.zip') -Force
Write-Host "应用分发包: $OutDir\hawk-windows-x64.zip"

Write-Host ""
Write-Host "完成：产物已归置到 $OutDir。" -ForegroundColor Green
