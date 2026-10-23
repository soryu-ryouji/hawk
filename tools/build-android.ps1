# 构建（Android）：构建 hawk-android 的 APK 并归置到输出目录（默认仓库根目录的 out/）。
# 统一入口 tools/build.ps1 --platform android。
#
# 用法: ./tools/build-android.ps1 [-Path <输出目录>] [-Release]
#   默认产出 debug APK（debug 签名，可直接 adb install）
#   -Release：产出 R8 压缩的 release APK（当前工程没有签名配置，产物未签名，不能直接安装）
# 前置: Android SDK Platform 37；JDK 21+（未设置 JAVA_HOME 时自动找 Android Studio 自带的 JBR）
#   SDK 位置：读取 hawk-android/local.properties，缺省时用 ANDROID_HOME / ANDROID_SDK_ROOT /
#   %LOCALAPPDATA%\Android\Sdk 并生成 local.properties

$ErrorActionPreference = 'Stop'

$Path = ''
$Release = $false
$argsList = @($args)
$i = 0
while ($i -lt $argsList.Count) {
    $a = [string]$argsList[$i]
    if ($a -match '^(-Path|--path)$') {
        if ($i + 1 -ge $argsList.Count) { throw "$a 需要目录参数" }
        $Path = [string]$argsList[$i + 1]; $i += 2
    } elseif ($a -match '^--?path=(.+)$') {
        $Path = $Matches[1]; $i++
    } elseif ($a -match '^(-Release|--release)$') {
        $Release = $true; $i++
    } else {
        throw "未知参数: $a（用法: ./tools/build-android.ps1 [-Path <输出目录>] [-Release]）"
    }
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
$AndroidDir = Join-Path $RepoRoot 'hawk-android'
$OutDir = if ($Path) { $Path } else { Join-Path $RepoRoot 'out' }

if (-not (Test-Path (Join-Path $AndroidDir 'gradlew.bat'))) {
    throw "未找到 hawk-android/gradlew.bat，仓库目录不完整"
}

function Test-JavaHome([string]$JdkHome) {
    if ([string]::IsNullOrWhiteSpace($JdkHome)) { return $false }
    return Test-Path (Join-Path $JdkHome 'bin\java.exe')
}

if (-not (Test-JavaHome $env:JAVA_HOME)) {
    $candidates = @()
    if ($env:ANDROID_STUDIO_JDK) { $candidates += $env:ANDROID_STUDIO_JDK }
    if ($env:ProgramFiles) { $candidates += Join-Path $env:ProgramFiles 'Android\Android Studio\jbr' }
    if (${env:ProgramFiles(x86)}) { $candidates += Join-Path ${env:ProgramFiles(x86)} 'Android\Android Studio\jbr' }
    if ($env:LOCALAPPDATA) {
        $candidates += Join-Path $env:LOCALAPPDATA 'Programs\Android Studio\jbr'
        $candidates += Join-Path $env:LOCALAPPDATA 'Android\Android Studio\jbr'
    }
    $found = $candidates | Where-Object { Test-JavaHome $_ } | Select-Object -First 1
    if ($found) {
        $env:JAVA_HOME = $found
        Write-Host "使用 JDK: $found"
    } elseif (-not (Get-Command java -ErrorAction SilentlyContinue)) {
        throw "未找到 JDK，请安装 Android Studio，或设置 JAVA_HOME 指向 JDK 21+"
    }
}

# Gradle 的 Android 插件需要 local.properties 或 ANDROID_HOME/ANDROID_SDK_ROOT。
$LocalProps = Join-Path $AndroidDir 'local.properties'
if (-not (Test-Path $LocalProps)) {
    $sdk = $env:ANDROID_HOME
    if (-not $sdk) { $sdk = $env:ANDROID_SDK_ROOT }
    if (-not $sdk) { $sdk = Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
    if (-not (Test-Path $sdk)) {
        throw "未找到 Android SDK，请安装 Android Studio，或设置 ANDROID_HOME/ANDROID_SDK_ROOT"
    }
    [System.IO.File]::WriteAllText($LocalProps, "sdk.dir=$($sdk.Replace('\', '/'))`n")
    Write-Host "已生成 local.properties: sdk.dir=$sdk"
}

Push-Location $AndroidDir
try {
    if ($Release) {
        & .\gradlew.bat :app:assembleRelease
    } else {
        & .\gradlew.bat :app:assembleDebug
    }
    if ($LASTEXITCODE -ne 0) { throw "Gradle 构建失败（exit $LASTEXITCODE）" }
} finally {
    Pop-Location
}

$apk = if ($Release) {
    'app\build\outputs\apk\release\app-release-unsigned.apk'
} else {
    'app\build\outputs\apk\debug\app-debug.apk'
}
$source = Join-Path $AndroidDir $apk
if (-not (Test-Path $source)) {
    throw "构建产物不存在: $source"
}

$name = if ($Release) { 'hawk-android-release-unsigned.apk' } else { 'hawk-android-debug.apk' }
New-Item -ItemType Directory -Force $OutDir | Out-Null
Copy-Item $source (Join-Path $OutDir $name) -Force
Write-Host ""
Write-Host "APK: $OutDir\$name"
if ($Release) {
    Write-Host "release 产物未签名，不能直接安装；调试请用默认的 debug 构建。" -ForegroundColor Yellow
}
Write-Host "完成：产物已归置到 $OutDir。" -ForegroundColor Green
