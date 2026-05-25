param(
    [string]$Scenario = "review-workspace",
    [Parameter(Mandatory = $true)]
    [string]$Output
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Try-SetScreenshotDisplayResolution {
    Add-Type -AssemblyName System.Windows.Forms
    $Bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    if ($Bounds.Width -ge 1440 -and $Bounds.Height -ge 1000) {
        return
    }

    if (Get-Command Set-DisplayResolution -ErrorAction SilentlyContinue) {
        Set-DisplayResolution -Width 1920 -Height 1200 -Force
    }
    else {
        Write-Warning "Primary display is $($Bounds.Width)x$($Bounds.Height), but Set-DisplayResolution is not available."
    }
}

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class RemissScreenshotWin32
{
    public const int SW_RESTORE = 9;
    public const int DWMWA_EXTENDED_FRAME_BOUNDS = 9;

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(
        IntPtr hWnd,
        IntPtr hWndInsertAfter,
        int X,
        int Y,
        int cx,
        int cy,
        uint uFlags
    );

    [DllImport("user32.dll")]
    public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);

    [DllImport("dwmapi.dll")]
    public static extern int DwmGetWindowAttribute(
        IntPtr hwnd,
        int dwAttribute,
        out RECT pvAttribute,
        int cbAttribute
    );
}
"@

function Get-RemissMainWindowHandle {
    param(
        [System.Diagnostics.Process]$Process
    )

    for ($i = 0; $i -lt 80; $i++) {
        $Process.Refresh()
        if ($Process.MainWindowHandle -ne [IntPtr]::Zero) {
            return $Process.MainWindowHandle
        }
        Start-Sleep -Milliseconds 250
    }

    throw "Remiss process did not expose a main window handle"
}

function Get-WindowBounds {
    param(
        [IntPtr]$Handle
    )

    $Rect = New-Object RemissScreenshotWin32+RECT
    $StructSize = [Runtime.InteropServices.Marshal]::SizeOf([type][RemissScreenshotWin32+RECT])
    $Result = [RemissScreenshotWin32]::DwmGetWindowAttribute(
        $Handle,
        [RemissScreenshotWin32]::DWMWA_EXTENDED_FRAME_BOUNDS,
        [ref]$Rect,
        $StructSize
    )

    if ($Result -ne 0 -or $Rect.Right -le $Rect.Left -or $Rect.Bottom -le $Rect.Top) {
        if (![RemissScreenshotWin32]::GetWindowRect($Handle, [ref]$Rect)) {
            throw "Could not read Remiss window bounds"
        }
    }

    return [System.Drawing.Rectangle]::FromLTRB($Rect.Left, $Rect.Top, $Rect.Right, $Rect.Bottom)
}

function Test-BitmapHasVariation {
    param(
        $Bitmap
    )

    $First = $Bitmap.GetPixel(0, 0).ToArgb()
    $StepX = [Math]::Max(1, [int]($Bitmap.Width / 12))
    $StepY = [Math]::Max(1, [int]($Bitmap.Height / 12))

    for ($Y = 0; $Y -lt $Bitmap.Height; $Y += $StepY) {
        for ($X = 0; $X -lt $Bitmap.Width; $X += $StepX) {
            if ($Bitmap.GetPixel($X, $Y).ToArgb() -ne $First) {
                return $true
            }
        }
    }

    return $false
}

function Try-CaptureWindowWithPrintWindow {
    param(
        [IntPtr]$Handle,
        $Bounds,
        [string]$OutputPath
    )

    $Bitmap = New-Object System.Drawing.Bitmap $Bounds.Width, $Bounds.Height
    $Graphics = [System.Drawing.Graphics]::FromImage($Bitmap)
    $Hdc = $Graphics.GetHdc()
    try {
        $Succeeded = [RemissScreenshotWin32]::PrintWindow($Handle, $Hdc, 0x00000002)
    }
    finally {
        $Graphics.ReleaseHdc($Hdc)
        $Graphics.Dispose()
    }

    try {
        if ($Succeeded -and (Test-BitmapHasVariation -Bitmap $Bitmap)) {
            $Bitmap.Save($OutputPath, [System.Drawing.Imaging.ImageFormat]::Png)
            return $true
        }

        return $false
    }
    finally {
        $Bitmap.Dispose()
    }
}

function Capture-Window {
    param(
        [IntPtr]$Handle,
        [string]$OutputPath
    )

    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing

    [RemissScreenshotWin32]::ShowWindow($Handle, [RemissScreenshotWin32]::SW_RESTORE) | Out-Null
    [RemissScreenshotWin32]::SetForegroundWindow($Handle) | Out-Null
    [RemissScreenshotWin32]::SetWindowPos($Handle, [IntPtr]::Zero, 24, 24, 0, 0, 0x0001 -bor 0x0004) | Out-Null
    Start-Sleep -Milliseconds 200

    $Bounds = Get-WindowBounds -Handle $Handle
    if (Try-CaptureWindowWithPrintWindow -Handle $Handle -Bounds $Bounds -OutputPath $OutputPath) {
        return
    }

    $ScreenBounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    if (
        $Bounds.Left -lt $ScreenBounds.Left -or
        $Bounds.Top -lt $ScreenBounds.Top -or
        $Bounds.Right -gt $ScreenBounds.Right -or
        $Bounds.Bottom -gt $ScreenBounds.Bottom
    ) {
        throw "Remiss window bounds $Bounds do not fit inside primary display bounds $ScreenBounds."
    }

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
}

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

Try-SetScreenshotDisplayResolution

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
    $Handle = Get-RemissMainWindowHandle -Process $Process
    Capture-Window -Handle $Handle -OutputPath $OutputPath

    if (!(Test-Path $OutputPath) -or (Get-Item $OutputPath).Length -le 0) {
        throw "Screenshot was not written: $OutputPath"
    }

    Write-Output $OutputPath
}
finally {
    Stop-Process -Id $Process.Id -ErrorAction SilentlyContinue
}
