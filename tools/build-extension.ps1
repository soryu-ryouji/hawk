# 发包（浏览器插件）：构建 Chrome / Firefox 插件并归置到输出目录
# （默认仓库根目录的 out/，即 out/hawk-extension-chrome|firefox/，浏览器「加载已解压的扩展程序」直接用）。
# 桌面应用是独立命令 tools/build-app.ps1；统一入口 tools/build.ps1 --platform extension。
#
# 用法: ./tools/build-extension.ps1 [-Path <输出目录>]
# 前置: 最新 Node.js（https://nodejs.org/）

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
        throw "未知参数: $a（用法: ./tools/build-extension.ps1 [-Path <输出目录>]）"
    }
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
$ExtDir = Join-Path $RepoRoot 'hawk-browser-extension'
$OutDir = if ($Path) { $Path } else { Join-Path $RepoRoot 'out' }

foreach ($tool in @('node', 'npm')) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "未找到 $tool，请先安装最新的 Node.js（https://nodejs.org/）"
    }
}

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
New-Item -ItemType Directory -Force $OutDir | Out-Null
robocopy $chromeOut (Join-Path $OutDir 'hawk-extension-chrome') /MIR /NFL /NDL /NJH /NJS | Out-Null
if ($LASTEXITCODE -gt 7) { throw "复制插件产物失败（robocopy exit $LASTEXITCODE）" }
robocopy $firefoxOut (Join-Path $OutDir 'hawk-extension-firefox') /MIR /NFL /NDL /NJH /NJS | Out-Null
if ($LASTEXITCODE -gt 7) { throw "复制插件产物失败（robocopy exit $LASTEXITCODE）" }
Write-Host "浏览器插件: $OutDir\hawk-extension-chrome、hawk-extension-firefox（浏览器「加载已解压的扩展程序」直接用）"

Write-Host ""
Write-Host "完成：产物已归置到 $OutDir。" -ForegroundColor Green
