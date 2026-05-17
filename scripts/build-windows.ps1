Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$ReleaseExe = Join-Path $Root "target\release\remiss.exe"
$Assets = Join-Path $Root "assets"
$Dist = Join-Path $Root "dist"
$Staging = Join-Path $Root ".build\windows\Remiss"

function Get-RemissVersion {
    if (-not [string]::IsNullOrWhiteSpace($env:REMISS_VERSION)) {
        return $env:REMISS_VERSION
    }

    $CargoToml = Join-Path $Root "Cargo.toml"
    $VersionLine = Get-Content $CargoToml | Where-Object { $_ -match '^version = "([^"]+)"' } | Select-Object -First 1
    if ($VersionLine -match '^version = "([^"]+)"') {
        return $Matches[1]
    }

    throw "Could not read package version from Cargo.toml"
}

function Get-RemissArch {
    $RawArch = $env:PROCESSOR_ARCHITECTURE
    if ([string]::IsNullOrWhiteSpace($RawArch)) {
        $RawArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    }

    switch -Regex ($RawArch) {
        '^(AMD64|X64)$' { return "x64" }
        '^ARM64$' { return "arm64" }
        default { return $RawArch.ToLowerInvariant() }
    }
}

function Set-GpuiFxcPath {
    if (-not [string]::IsNullOrWhiteSpace($env:GPUI_FXC_PATH)) {
        return
    }

    $Command = Get-Command fxc.exe -ErrorAction SilentlyContinue
    if ($Command) {
        $env:GPUI_FXC_PATH = $Command.Source
        return
    }

    $ProgramFilesX86 = ${env:ProgramFiles(x86)}
    if ([string]::IsNullOrWhiteSpace($ProgramFilesX86)) {
        return
    }

    $KitsRoot = Join-Path $ProgramFilesX86 "Windows Kits\10\bin"
    if (-not (Test-Path $KitsRoot)) {
        return
    }

    $Candidate = Get-ChildItem -Path $KitsRoot -Recurse -Filter fxc.exe |
        Where-Object { $_.FullName -match '\\x64\\fxc\.exe$' } |
        Sort-Object FullName |
        Select-Object -Last 1

    if ($Candidate) {
        $env:GPUI_FXC_PATH = $Candidate.FullName
    }
}

$Version = Get-RemissVersion
$Arch = Get-RemissArch
$Zip = Join-Path $Dist "remiss-$Version-windows-$Arch.zip"

Set-GpuiFxcPath

Push-Location $Root
try {
    cargo build --release
}
finally {
    Pop-Location
}

if (-not (Test-Path $ReleaseExe)) {
    throw "Expected release executable at $ReleaseExe"
}

Remove-Item -Recurse -Force $Staging -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Staging | Out-Null
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

Copy-Item $ReleaseExe (Join-Path $Staging "Remiss.exe")
Copy-Item $Assets (Join-Path $Staging "assets") -Recurse
Get-ChildItem (Join-Path $Staging "assets") -Recurse -Force -Include ".DS_Store", "._*" |
    Remove-Item -Force

Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $Staging "*") -DestinationPath $Zip -Force

Write-Output $Zip
