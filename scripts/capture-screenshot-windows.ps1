param(
    [string]$Scenario = "review-workspace",
    [Parameter(Mandatory = $true)]
    [string]$Output
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = [string](Resolve-Path (Join-Path $PSScriptRoot ".."))
$StagedExe = Join-Path $Root ".build\windows\Remiss\Remiss.exe"
$ReleaseExe = Join-Path $Root "target\release\remiss.exe"

if (Test-Path $StagedExe) {
    $Executable = [string](Resolve-Path $StagedExe)
}
elseif (Test-Path $ReleaseExe) {
    $Executable = [string](Resolve-Path $ReleaseExe)
}
else {
    throw "Expected Remiss executable at $StagedExe. Run .\scripts\build-windows.ps1 first."
}

$OutputPath = [System.IO.Path]::GetFullPath($Output)
$OutputDir = Split-Path -Parent $OutputPath
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$ReadyFile = "$OutputPath.ready"
Remove-Item -Force $OutputPath, $ReadyFile -ErrorAction SilentlyContinue

$env:REMISS_SCREENSHOT_MODE = "1"
$env:REMISS_SCREENSHOT_SCENARIO = $Scenario
$env:REMISS_SCREENSHOT_OUTPUT_FILE = $OutputPath

$Process = Start-Process -FilePath $Executable -WorkingDirectory (Split-Path -Parent $Executable) -PassThru

try {
    for ($i = 0; $i -lt 160; $i++) {
        if (Test-Path $ReadyFile) {
            break
        }
        Start-Sleep -Milliseconds 250
    }

    if (!(Test-Path $ReadyFile)) {
        throw "Remiss did not become screenshot-ready: $ReadyFile"
    }

    Start-Sleep -Milliseconds 200
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing

    $Bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $Bitmap = New-Object System.Drawing.Bitmap $Bounds.Width, $Bounds.Height
    $Graphics = [System.Drawing.Graphics]::FromImage($Bitmap)
    try {
        $Graphics.CopyFromScreen($Bounds.Location, [System.Drawing.Point]::Empty, $Bounds.Size)
        $Bitmap.Save($OutputPath, [System.Drawing.Imaging.ImageFormat]::Png)
    }
    finally {
        $Graphics.Dispose()
        $Bitmap.Dispose()
    }

    if (!(Test-Path $OutputPath) -or (Get-Item $OutputPath).Length -le 0) {
        throw "Screenshot was not written: $OutputPath"
    }

    Write-Output $OutputPath
}
finally {
    Stop-Process -Id $Process.Id -ErrorAction SilentlyContinue
}
