# 统一发包入口：--platform 选择目标，转发到对应构建脚本（编译实现只在 build-app / build-extension 一处）。
# 用法: ./tools/build.ps1 --platform <app|extension> [-Path <输出目录>]
#   ./tools/build.ps1 --platform app               # 桌面应用 → out/hawk-windows-x64.zip
#   ./tools/build.ps1 --platform extension         # 浏览器插件 → out/hawk-extension-chrome|firefox/
#   ./tools/build.ps1 --platform extension --path D:/publish   # 指定输出目录（--path= 写法亦可）

$ErrorActionPreference = 'Stop'

$Platform = ''
$Path = ''
$argsList = @($args)
$i = 0
while ($i -lt $argsList.Count) {
    $a = [string]$argsList[$i]
    if ($a -match '^(-Platform|--platform)$') {
        if ($i + 1 -ge $argsList.Count) { throw "$a 需要参数（app|extension）" }
        $Platform = [string]$argsList[$i + 1]; $i += 2
    } elseif ($a -match '^--?platform=(.+)$') {
        $Platform = $Matches[1]; $i++
    } elseif ($a -match '^(-Path|--path)$') {
        if ($i + 1 -ge $argsList.Count) { throw "$a 需要目录参数" }
        $Path = [string]$argsList[$i + 1]; $i += 2
    } elseif ($a -match '^--?path=(.+)$') {
        $Path = $Matches[1]; $i++
    } else {
        throw "未知参数: $a（用法: ./tools/build.ps1 --platform <app|extension> [-Path <输出目录>]）"
    }
}

if (-not $Platform) {
    throw "缺少 --platform（用法: ./tools/build.ps1 --platform <app|extension> [-Path <输出目录>]）"
}
if ($Platform -notin @('app', 'extension')) {
    throw "未知 --platform: $Platform（支持 app|extension）"
}

$child = if ($Platform -eq 'app') { 'build-app.ps1' } else { 'build-extension.ps1' }
if ($Path) {
    & (Join-Path $PSScriptRoot $child) -Path $Path
} else {
    & (Join-Path $PSScriptRoot $child)
}
