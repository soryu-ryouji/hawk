# 本机安装：构建 hawk 桌面应用，把可运行文件归置到目标目录（默认仓库根目录的 out/）。
# Windows 产物为免安装目录（hawk.exe 就地可运行），复制目录内容到任意位置即可使用。
#
# 用法: ./tools/install.ps1 [-Path <输出目录>]
#   ./tools/install.ps1                      # → <仓库>/out/
#   ./tools/install.ps1 -Path D:/Tools/hawk  # → D:/Tools/hawk（就地可运行）
# 前置: 最新 Node.js 与 Rust 工具链（https://rustup.rs/）
param([string]$Path = "")

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$AppDir = Join-Path $RepoRoot 'hawk-app'
$OutDir = if ($Path) { $Path } else { Join-Path $RepoRoot 'out' }

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

$unpacked = Join-Path $AppDir 'dist\win-unpacked'
if (-not (Test-Path $unpacked)) {
    throw "打包产物不存在: $unpacked（electron-builder 未产出 win-unpacked）"
}

# 复制（仅变化文件，不删除目标目录里已有内容）；robocopy 退出码 0-7 均为成功
robocopy $unpacked $OutDir /E /NFL /NDL /NJH /NJS | Out-Null
if ($LASTEXITCODE -gt 7) {
    throw "复制到 $OutDir 失败（robocopy exit $LASTEXITCODE）"
}

Write-Host ""
Write-Host "完成：应用已归置到 $OutDir（hawk.exe 就地可运行）。" -ForegroundColor Green
