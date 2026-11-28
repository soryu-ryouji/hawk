param(
    [Parameter(Mandatory = $true)][int]$WaitPid,
    [Parameter(Mandatory = $true)][string]$Zip,
    [Parameter(Mandatory = $true)][string]$AppDir,
    [string]$Exe = 'hawk.exe'
)

$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $env:TEMP

$staging = $null
$backup = $null
$LogDir = Join-Path $env:TEMP 'hawk-update'
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$Log = Join-Path $LogDir 'install.log'

function Write-Log([string]$Message) {
    Add-Content -LiteralPath $Log -Value ('[{0}] {1}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Message) -Encoding UTF8
}

function Show-Failure([string]$Message) {
    Write-Log ('ERROR: ' + $Message)
    $text = $Message + "`r`n`r`nLog: " + $Log
    try {
        $shell = New-Object -ComObject WScript.Shell
        $shell.Popup($text, 60, 'hawk update failed', 16) | Out-Null
    } catch {
        try {
            Add-Type -AssemblyName PresentationFramework
            [System.Windows.MessageBox]::Show($text, 'hawk update failed', 'OK', 'Error') | Out-Null
        } catch {
            # Headless/session-isolated environments cannot show a dialog; the log is the fallback.
        }
    }
}

function Invoke-WithRetry([scriptblock]$Action, [string]$What) {
    for ($attempt = 1; $attempt -le 10; $attempt++) {
        try {
            & $Action
            return
        } catch {
            if ($attempt -eq 10) {
                throw
            }
            Write-Log ('retry {0} ({1}/10): {2}' -f $What, $attempt, $_.Exception.Message)
            Start-Sleep -Milliseconds 500
        }
    }
}

function Wait-ForProcessExit([int]$ProcessId) {
    $deadline = (Get-Date).AddSeconds(120)
    while ((Get-Date) -lt $deadline) {
        $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
        if (-not $process) {
            return
        }
        Start-Sleep -Milliseconds 300
    }
    throw ('Timed out waiting for process {0} to exit' -f $ProcessId)
}

function Wait-ForAppProcesses([string]$Directory) {
    # The daemon and Electron utility processes also lock files under the app directory.
    $deadline = (Get-Date).AddSeconds(90)
    while ((Get-Date) -lt $deadline) {
        $remaining = @()
        foreach ($process in Get-Process -ErrorAction SilentlyContinue) {
            try {
                $path = $process.Path
                if ($path -and $path.StartsWith($Directory + '\', [StringComparison]::OrdinalIgnoreCase)) {
                    $remaining += ('{0}({1})' -f $process.ProcessName, $process.Id)
                }
            } catch {
                # Access denied for protected/system processes; ignore.
            }
        }
        if ($remaining.Count -eq 0) {
            return
        }
        Write-Log ('waiting for app processes: ' + ($remaining -join ', '))
        Start-Sleep -Milliseconds 500
    }
    throw 'Timed out waiting for app processes to exit'
}

function Remove-PathQuietly([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) {
        return
    }
    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue
    }
}

try {
    Write-Log ('starting update: pid={0} zip={1} app={2}' -f $WaitPid, $Zip, $AppDir)

    $AppDir = (Resolve-Path -LiteralPath $AppDir).Path.TrimEnd('\')
    if (-not (Test-Path -LiteralPath $Zip -PathType Leaf)) {
        throw ('update package not found: ' + $Zip)
    }
    if (-not (Test-Path -LiteralPath (Join-Path $AppDir $Exe) -PathType Leaf)) {
        throw ('app executable not found: ' + (Join-Path $AppDir $Exe))
    }

    Wait-ForProcessExit $WaitPid
    Wait-ForAppProcesses $AppDir
    Start-Sleep -Milliseconds 500

    $parent = Split-Path -Parent $AppDir
    $staging = Join-Path $parent ('.hawk-update-' + [guid]::NewGuid().ToString('N'))
    $backup = $AppDir + '.old-' + (Get-Date -Format 'yyyyMMddHHmmss')
    Remove-PathQuietly $staging

    # A zip downloaded by a browser may carry Mark-of-the-Web. Remove it before
    # extraction so the new executable does not block on "Open File - Security Warning".
    try {
        Unblock-File -LiteralPath $Zip -ErrorAction SilentlyContinue
    } catch {
        Write-Log ('unblock zip failed: ' + $_.Exception.Message)
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    Write-Log ('extracting ' + $Zip)
    [System.IO.Compression.ZipFile]::ExtractToDirectory($Zip, $staging)

    $sourceRoot = $staging
    if (-not (Test-Path -LiteralPath (Join-Path $sourceRoot $Exe) -PathType Leaf)) {
        $found = Get-ChildItem -LiteralPath $staging -Filter $Exe -Recurse -File | Select-Object -First 1
        if (-not $found) {
            throw ('update package does not contain ' + $Exe)
        }
        $sourceRoot = $found.Directory.FullName
    }

    if (Test-Path -LiteralPath $backup) {
        $backup = $AppDir + '.old-' + [guid]::NewGuid().ToString('N')
    }

    $replaced = $false
    $started = $false
    try {
        Invoke-WithRetry { Rename-Item -LiteralPath $AppDir -NewName (Split-Path -Leaf $backup) } 'rename app directory'
        Invoke-WithRetry { Move-Item -LiteralPath $sourceRoot -Destination $AppDir } 'move new version'
        $replaced = $true
        try {
            Unblock-File -LiteralPath (Join-Path $AppDir $Exe) -ErrorAction SilentlyContinue
        } catch {
            Write-Log ('unblock exe failed: ' + $_.Exception.Message)
        }

        $newProcess = Start-Process -FilePath (Join-Path $AppDir $Exe) -WorkingDirectory $AppDir -PassThru
        Start-Sleep -Seconds 2
        if ($newProcess.HasExited) {
            throw ('new version exited immediately (code ' + $newProcess.ExitCode + ')')
        }
        $started = $true
        Write-Log 'update complete'
        Remove-PathQuietly $backup
        Remove-PathQuietly $staging
        exit 0
    } catch {
        $message = $_.Exception.Message
        if ($replaced -and -not $started) {
            # Keep the new tree aside, restore the previous version, and start it.
            try {
                $failed = $AppDir + '.failed-' + [guid]::NewGuid().ToString('N')
                if (Test-Path -LiteralPath $AppDir) {
                    Move-Item -LiteralPath $AppDir -Destination $failed
                }
                if (Test-Path -LiteralPath $backup) {
                    Move-Item -LiteralPath $backup -Destination $AppDir
                    if (Test-Path -LiteralPath (Join-Path $AppDir $Exe)) {
                        Start-Process -FilePath (Join-Path $AppDir $Exe) -WorkingDirectory $AppDir | Out-Null
                    }
                }
            } catch {
                $message += '; rollback failed: ' + $_.Exception.Message
            }
        } elseif (-not $replaced -and (Test-Path -LiteralPath (Join-Path $AppDir $Exe))) {
            # Nothing was replaced yet; bring the previous version back up.
            try {
                Start-Process -FilePath (Join-Path $AppDir $Exe) -WorkingDirectory $AppDir | Out-Null
            } catch {
                $message += '; restart failed: ' + $_.Exception.Message
            }
        }
        Show-Failure $message
        exit 1
    }
} catch {
    $message = $_.Exception.Message
    try {
        if ($backup -and (Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $AppDir)) {
            Move-Item -LiteralPath $backup -Destination $AppDir
        }
    } catch {
        $message += '; rollback failed: ' + $_.Exception.Message
    }
    if (Test-Path -LiteralPath (Join-Path $AppDir $Exe)) {
        try {
            Start-Process -FilePath (Join-Path $AppDir $Exe) -WorkingDirectory $AppDir | Out-Null
        } catch {
            $message += '; restart failed: ' + $_.Exception.Message
        }
    }
    Show-Failure $message
    exit 1
} finally {
    Remove-PathQuietly $staging
}
