# 本机安装：构建 hawk 桌面应用，把可运行文件归置到目标目录（默认仓库根目录的 out/）。
# Windows 产物为免安装目录（hawk.exe 就地可运行）。编译复用 build-app.ps1（--unpacked 模式跳过 zip 压缩）。
#
# 用法: ./tools/install.ps1 [-Path <输出目录>]（--path / --path= 写法亦可）
#   ./tools/install.ps1                      # → <仓库>/out/
#   ./tools/install.ps1 -Path D:/Tools/hawk  # → D:/Tools/hawk（就地可运行）
# 前置: 最新 Node.js 与 Rust 工具链（https://rustup.rs/）

$ErrorActionPreference = 'Stop'

$Path = ''
$argsList = @($args)
$i = 0
while ($i -lt $argsList.Count) {
    $a = [string]$argsList[$i]
    if ($a -match '^(-Path|--path)$') {
        if ($i + 1 -ge $argsList.Count) { throw "$a 需要目录参数" }
        $Path = [string]$argsList[$i + 1]; $i += 2
    } elseif ($a -match '^--?path=(.+)$') {
        $Path = $Matches[1]; $i++
    } else {
        throw "未知参数: $a（用法: ./tools/install.ps1 [-Path <输出目录>]）"
    }
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
$OutDir = if ($Path) { $Path } else { Join-Path $RepoRoot 'out' }

# 编译委托给 build-app.ps1（工具链检查 / 依赖安装 / 镜像配置都在那边）
& (Join-Path $PSScriptRoot 'build-app.ps1') -Unpacked

$unpacked = Join-Path $RepoRoot 'hawk-app\dist\win-unpacked'
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
