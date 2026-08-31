# 发包：构建 hawk 桌面应用的分发包并归置到仓库根目录的 out/（Windows 为免安装 hawk.zip）。
# -Extensions：附带构建浏览器插件（out/hawk-extension-chrome|firefox/，加载已解压扩展即用）。
#
# 用法: ./tools/build.ps1 [-Extensions]
# 前置: 最新 Node.js 与 Rust 工具链（https://rustup.rs/）
# 压缩级别（默认 5）: ELECTRON_BUILDER_COMPRESSION_LEVEL=9 ./tools/build.ps1   # 9=最小体积，3=最快
param([switch]$Extensions)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$AppDir = Join-Path $RepoRoot 'hawk-app'
$ExtDir = Join-Path $RepoRoot 'hawk-browser-extension'
$OutDir = Join-Path $RepoRoot 'out'

foreach ($tool in @('node', 'npm', 'cargo')) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "未找到 $tool，请先安装最新的 Node.js 与 Rust 工具链（https://rustup.rs/）"
    }
}

Push-Location $AppDir
try {
    if (-not (Test-Path 'node_modules')) {
        npm install
    }
    npm run pack
} finally {
    Pop-Location
}

$package = Join-Path $AppDir 'dist\hawk.zip'
if (-not (Test-Path $package)) {
    throw "打包产物不存在: $package（electron-builder 未产出 hawk.zip）"
}
New-Item -ItemType Directory -Force $OutDir | Out-Null
Copy-Item $package (Join-Path $OutDir 'hawk.zip') -Force
Write-Host "应用分发包: $OutDir\hawk.zip"

if ($Extensions) {
    Push-Location $ExtDir
    try {
        if (-not (Test-Path 'node_modules')) {
            npm install
        }
        npm run build
        npm run build:firefox
    } finally {
        Pop-Location
    }
    $chromeOut = Join-Path $ExtDir '.output\chrome-mv3'
    $firefoxOut = Join-Path $ExtDir '.output\firefox-mv2'
    if (-not (Test-Path $chromeOut) -or -not (Test-Path $firefoxOut)) {
        throw "插件构建产物不存在: .output/chrome-mv3 或 .output/firefox-mv2"
    }
    # 插件目录独立，镜像同步避免旧版本残留
    robocopy $chromeOut (Join-Path $OutDir 'hawk-extension-chrome') /MIR /NFL /NDL /NJH /NJS | Out-Null
    if ($LASTEXITCODE -gt 7) { throw "复制插件产物失败（robocopy exit $LASTEXITCODE）" }
    robocopy $firefoxOut (Join-Path $OutDir 'hawk-extension-firefox') /MIR /NFL /NDL /NJH /NJS | Out-Null
    if ($LASTEXITCODE -gt 7) { throw "复制插件产物失败（robocopy exit $LASTEXITCODE）" }
    Write-Host "浏览器插件: $OutDir\hawk-extension-chrome、hawk-extension-firefox（浏览器「加载已解压的扩展程序」直接用）"
}

Write-Host ""
Write-Host "完成：全部产物已归置到 $OutDir。" -ForegroundColor Green
