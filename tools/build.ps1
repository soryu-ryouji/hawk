# 统一发包入口：--platform 选择目标，转发到对应构建脚本（编译实现只在 build-app / build-extension / build-android 一处）。
# 用法: ./tools/build.ps1 --platform <app|extension|android> [-Path <输出目录>] [-Release]
#   ./tools/build.ps1 --platform app               # 桌面应用 → out/hawk-windows-x64.zip
#   ./tools/build.ps1 --platform extension         # 浏览器插件 → out/hawk-extension-chrome|firefox/
#   ./tools/build.ps1 --platform android           # Android debug APK → out/hawk-android-debug.apk
#   ./tools/build.ps1 --platform android -Release  # Android release APK（未签名）→ out/
#   ./tools/build.ps1 --platform extension --path D:/publish   # 指定输出目录（--path= 写法亦可）

$ErrorActionPreference = 'Stop'

$Platform = ''
$Path = ''
$Release = $false
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
    } elseif ($a -match '^(-Release|--release)$') {
        $Release = $true; $i++
    } else {
        throw "未知参数: $a（用法: ./tools/build.ps1 --platform <app|extension|android> [-Path <输出目录>] [-Release]）"
    }
}

if (-not $Platform) {
    throw "缺少 --platform（用法: ./tools/build.ps1 --platform <app|extension|android> [-Path <输出目录>] [-Release]）"
}
if ($Platform -notin @('app', 'extension', 'android')) {
    throw "未知 --platform: $Platform（支持 app|extension|android）"
}
if ($Release -and $Platform -ne 'android') {
    throw "-Release 仅适用于 --platform android"
}

$child = switch ($Platform) {
    'app' { 'build-app.ps1' }
    'extension' { 'build-extension.ps1' }
    'android' { 'build-android.ps1' }
}
$script = Join-Path $PSScriptRoot $child
# 显式转发：子脚本用 $args 手工解析，splat 会拼出 "-Path:xxx" 这种单参数形式
if ($Platform -eq 'android') {
    if ($Path -and $Release) {
        & $script -Path $Path -Release
    } elseif ($Path) {
        & $script -Path $Path
    } elseif ($Release) {
        & $script -Release
    } else {
        & $script
    }
} elseif ($Path) {
    & $script -Path $Path
} else {
    & $script
}
