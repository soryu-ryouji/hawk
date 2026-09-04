# 本机安装：构建 hawk 桌面应用，把可运行文件归置到目标目录（默认仓库根目录的 out/）。
# Windows 产物为免安装目录（hawk.exe 就地可运行）。
#
# 用法: ./tools/install.ps1 [-Path <输出目录>]（--path / --path= 写法亦可）
#   ./tools/install.ps1                      # → <仓库>/out/
#   ./tools/install.ps1 -Path D:/Tools/hawk  # → D:/Tools/hawk（就地可运行）
# 前置: 最新 Node.js 与 Rust 工具链（https://rustup.rs/）
param([string]$Path = "")

$ErrorActionPreference = 'Stop'

# 兼容 --path <dir> / --path=<dir>：PowerShell 不会把双横线绑定到参数名，
# 此时 "--path" 被当作 $Path 的位置值，真正的目录落在 $args
if ($Path -match '^--?path$' -and $args.Count -ge 1) {
    $Path = [string]$args[0]
} elseif ($Path -match '^--?path=(.+)$') {
    $Path = $Matches[1]
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
    # pack:dir 跳过 zip 压缩——install 只需要未打包目录
    npm run pack:dir
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
