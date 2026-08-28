# 统一构建入口：把指定内容构建并拷贝到目标目录。
#
# 用法（- / -- / 写法均可，--platform 的值用逗号分隔）：
#   ./tools/build.ps1                                          # 构建全部 → <仓库>/out/
#   ./tools/build.ps1 --platform app --path D:/Tools/hawk      # 只构建桌面应用
#   ./tools/build.ps1 --platform ext-chrome,ext-firefox        # 浏览器插件（构建尚未实现，占位跳过）
#   ./tools/build.ps1 --platform=app --path=D:/Tools/hawk      # = 写法等价
#
# 产物直接输出到 <path>/ 根目录，不再套子文件夹：
#   应用（当前平台）：Windows 解压后的应用文件（hawk.exe 就地可运行）/ macOS hawk.app 目录 / Linux hawk.AppImage
#   浏览器插件：hawk-extension-chrome|firefox/ 目录（已解压的扩展，浏览器「加载已解压扩展程序」直接用）
#   Safari 版需 macOS + Xcode 转换（见 hawk-browser-extension/README.md），不在本脚本构建
#
# 退出码：0 = 请求的内容全部构建成功；1 = 存在未构建成功（失败或跳过）的内容。

$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$AppDir = Join-Path $RepoRoot 'hawk-app'
$ValidPlatforms = @('app', 'ext-chrome', 'ext-firefox')

# ---- 参数解析 ----
# PowerShell 不会把双横线参数（--platform）绑定到任何变量，统一手工解析 $args。
$Platform = $null
$Path = $null

for ($i = 0; $i -lt $args.Count; $i++) {
    $token = $args[$i]
    $name = $null
    $value = $null
    if ($token -is [string] -and ($token -match '^-{1,2}([A-Za-z]+)(?:=(.*))?$' -or $token -match '^/([A-Za-z]+)(?:=(.*))?$')) {
        $name = $Matches[1].ToLowerInvariant()
        $value = $Matches[2]
    }
    if ($null -eq $name) {
        Write-Host "无法识别的参数：$token（用法见文件头注释）" -ForegroundColor Red
        exit 1
    }
    if ($name -notin @('platform', 'path')) {
        Write-Host "未知参数：-$name（可选：-platform / -path）" -ForegroundColor Red
        exit 1
    }
    if ($null -eq $value) {
        if ($i + 1 -ge $args.Count) {
            Write-Host "参数 -$name 缺少值" -ForegroundColor Red
            exit 1
        }
        $token = $args[++$i]
        # pwsh 会把 a,b 先展开成数组再传入，还原为逗号分隔的单个字符串
        if ($token -is [array]) { $value = ($token | ForEach-Object { [string]$_ }) -join ',' }
        else { $value = [string]$token }
    }
    switch ($name) {
        'platform' { $Platform = $value -split ',' | ForEach-Object { $_.Trim() } }
        'path' { $Path = $value }
    }
}

if (-not $Platform) { $Platform = @('app', 'ext-chrome', 'ext-firefox') }
if (-not $Path) { $Path = Join-Path $PSScriptRoot '..\out' }

# ---- 参数校验 ----
foreach ($p in $Platform) {
    if ($ValidPlatforms -notcontains $p) {
        Write-Host "未知平台：$p（可选：$($ValidPlatforms -join ' / ')）" -ForegroundColor Red
        exit 1
    }
}

New-Item -ItemType Directory -Force -Path $Path | Out-Null
$outRoot = (Resolve-Path $Path).Path

$failed = @()

# 删除目标路径下与产物同名的旧文件/目录，避免残留旧产物
function Remove-StaleArtifact([string]$target) {
    if (Test-Path $target) { Remove-Item $target -Recurse -Force }
}

# ---- 桌面应用（hawk-app）----
function Build-App {
    Write-Host "`n==> [app] 构建 hawk-app（前端 + hawk-server + electron 打包）" -ForegroundColor Cyan

    if (-not (Test-Path (Join-Path $AppDir 'node_modules'))) {
        throw 'hawk-app/node_modules 不存在，请先执行：cd hawk-app && npm install'
    }

    Push-Location $AppDir
    try {
        & npm run pack
        if ($LASTEXITCODE -ne 0) { throw "npm run pack 失败（退出码 $LASTEXITCODE）" }
    }
    finally {
        Pop-Location
    }

    # electron-builder 产物在 hawk-app/dist：
    #   Windows: hawk.zip（zip 目标，真身 exe 在其根目录）；macOS: mac*/hawk.app 目录；Linux: hawk.AppImage
    $dist = Join-Path $AppDir 'dist'
    $isWin = ($IsWindows -eq $true) -or $env:OS -eq 'Windows_NT'
    $artifact = $null
    if ($isWin) {
        $artifact = Get-ChildItem $dist -Recurse -File -Filter 'hawk.zip' |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
    }
    elseif ($IsMacOS -eq $true) {
        $artifact = Get-ChildItem $dist -Recurse -Directory -Filter 'hawk.app' |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
    }
    else {
        $artifact = Get-ChildItem $dist -Recurse -File -Filter 'hawk.AppImage' |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
    }
    if (-not $artifact) { throw "未在 $dist 找到当前平台的打包产物" }

    if ($isWin) {
        # Windows：解压 zip 到目标目录，hawk.exe 就地可运行（本地测试免二次解压）。
        # 先按 zip 顶层条目清掉旧产物再解压，避免已删除的文件残留。
        # 用系统内置 bsdtar（System32\tar.exe）：PATH 里的 GNU tar（Git 自带）不支持 zip
        $bsdtar = Join-Path $env:SystemRoot 'System32\tar.exe'
        $topLevel = & $bsdtar -tf $artifact.FullName | ForEach-Object { ($_ -split '/')[0] } | Sort-Object -Unique
        foreach ($name in $topLevel) {
            if ($name -and $name -ne '.' -and $name -ne '..') {
                Remove-StaleArtifact (Join-Path $outRoot $name)
            }
        }
        & $bsdtar -xf $artifact.FullName -C $outRoot
        if ($LASTEXITCODE -ne 0) { throw "解压 $($artifact.Name) 失败（退出码 $LASTEXITCODE）" }
        Write-Host "[app] 已解压 → $outRoot（hawk.exe 可直接运行）" -ForegroundColor Green
        return
    }

    # 其余平台：产物直接平铺到 <path>/ 根目录
    $target = Join-Path $outRoot $artifact.Name
    Remove-StaleArtifact $target
    if ($artifact.PSIsContainer) {
        Copy-Item $artifact.FullName $target -Recurse
    }
    else {
        Copy-Item $artifact.FullName $target
    }
    Write-Host "[app] 已输出 → $target" -ForegroundColor Green
}

# ---- 浏览器插件（hawk-browser-extension，WXT 跨浏览器构建）----
function Build-Extension([string]$Browser) {
    Write-Host "`n==> [ext-$Browser] 构建浏览器插件" -ForegroundColor Cyan

    $extDir = Join-Path $RepoRoot 'hawk-browser-extension'
    if (-not (Test-Path (Join-Path $extDir 'node_modules'))) {
        throw 'hawk-browser-extension/node_modules 不存在，请先执行：cd hawk-browser-extension && npm install'
    }

    # WXT 输出目录：Chrome 为 MV3，Firefox/Safari 为 MV2
    $outDirName = switch ($Browser) {
        'chrome' { 'chrome-mv3' }
        'firefox' { 'firefox-mv2' }
        default { throw "不支持的浏览器: $Browser" }
    }
    $scriptName = if ($Browser -eq 'chrome') { 'build' } else { "build:$Browser" }

    Push-Location $extDir
    try {
        & npm run $scriptName
        if ($LASTEXITCODE -ne 0) { throw "npm run $scriptName 失败（退出码 $LASTEXITCODE）" }
    }
    finally {
        Pop-Location
    }

    $src = Join-Path $extDir ".output/$outDirName"
    if (-not (Test-Path $src)) { throw "未找到插件构建产物: $src" }

    $target = Join-Path $outRoot "hawk-extension-$Browser"
    Remove-StaleArtifact $target
    Copy-Item $src $target -Recurse
    Write-Host "[ext-$Browser] 已输出 → $target（浏览器加载已解压扩展即可）" -ForegroundColor Green
}

# ---- 主流程 ----
foreach ($p in $Platform) {
    switch ($p) {
        'app' {
            try {
                Build-App
            }
            catch {
                Write-Host "`n[app] 构建失败：$($_.Exception.Message)" -ForegroundColor Red
                $script:failed += 'app'
            }
        }
        'ext-chrome' {
            try {
                Build-Extension 'chrome'
            }
            catch {
                Write-Host "`n[ext-chrome] 构建失败：$($_.Exception.Message)" -ForegroundColor Red
                $script:failed += 'ext-chrome'
            }
        }
        'ext-firefox' {
            try {
                Build-Extension 'firefox'
            }
            catch {
                Write-Host "`n[ext-firefox] 构建失败：$($_.Exception.Message)" -ForegroundColor Red
                $script:failed += 'ext-firefox'
            }
        }
    }
}

Write-Host "`n产物目录：$outRoot"
if ($failed) {
    Write-Host "未构建成功：$($failed -join '、')" -ForegroundColor Red
    exit 1
}
Write-Host '全部构建完成' -ForegroundColor Green
