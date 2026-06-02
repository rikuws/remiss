Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$Staging = Join-Path $Root ".build\windows\Remiss"
$VelopackOut = Join-Path $Root "dist\velopack"
$ReleaseNotes = Join-Path $Root ".build\windows\velopack-release-notes.md"
$VpkVersion = "0.0.1298"

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

function Ensure-Vpk {
    dotnet tool update --global vpk --version $VpkVersion
    $DotnetTools = Join-Path $HOME ".dotnet\tools"
    if ($env:PATH -notlike "*$DotnetTools*") {
        $env:PATH = "$DotnetTools;$env:PATH"
    }
}

if (-not (Test-Path (Join-Path $Staging "Remiss.exe"))) {
    throw "Expected staged Remiss.exe at $Staging. Run scripts\build-windows.ps1 first."
}

$Version = Get-RemissVersion
Ensure-Vpk

Remove-Item -Recurse -Force $VelopackOut -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $VelopackOut | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path $ReleaseNotes -Parent) | Out-Null
"Automated Remiss build $Version." | Set-Content -Path $ReleaseNotes

$PackArgs = @(
    "pack",
    "--packId", "Remiss",
    "--packVersion", $Version,
    "--packDir", $Staging,
    "--mainExe", "Remiss.exe",
    "--packTitle", "Remiss",
    "--packAuthors", "Riku Wikman",
    "--releaseNotes", $ReleaseNotes,
    "--outputDir", $VelopackOut
)

if (-not [string]::IsNullOrWhiteSpace($env:REMISS_WINDOWS_AZURE_TRUSTED_SIGN_FILE)) {
    $PackArgs += @("--azureTrustedSignFile", $env:REMISS_WINDOWS_AZURE_TRUSTED_SIGN_FILE)
} elseif (-not [string]::IsNullOrWhiteSpace($env:REMISS_WINDOWS_SIGN_PARAMS)) {
    $PackArgs += @("--signParams", $env:REMISS_WINDOWS_SIGN_PARAMS)
}

vpk --yes --skip-updates @PackArgs

Get-ChildItem -Path $VelopackOut -File | ForEach-Object { $_.FullName }
