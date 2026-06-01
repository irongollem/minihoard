# minihoard installer for Windows
# Usage: irm https://github.com/irongollem/minihoard/releases/latest/download/install.ps1 | iex
# Override install dir: $env:BIN_DIR = "C:\tools"; iex (irm ...)
$ErrorActionPreference = 'Stop'

$Repo    = "irongollem/minihoard"
$BinDir  = if ($env:BIN_DIR) { $env:BIN_DIR } else { "$env:LOCALAPPDATA\minihoard\bin" }
$Target  = "x86_64-pc-windows-msvc"

$Release = (Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest").tag_name
if (-not $Release) {
    Write-Error "Could not fetch latest release. Check your internet connection."
    exit 1
}

Write-Host "Installing minihoard $Release to $BinDir"
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

foreach ($Bin in @("minihoard", "minihoard-mcp")) {
    $Url  = "https://github.com/$Repo/releases/download/$Release/${Bin}-${Target}.exe"
    $Dest = Join-Path $BinDir "$Bin.exe"
    Write-Host "  $Bin..."
    Invoke-WebRequest -Uri $Url -OutFile $Dest
}

# Add to user PATH if not already present
$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($UserPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("PATH", "$UserPath;$BinDir", "User")
    Write-Host ""
    Write-Host "Added $BinDir to your PATH (restart your terminal to apply)."
}

Write-Host ""
Write-Host "Installed:"
Write-Host "  $BinDir\minihoard.exe"
Write-Host "  $BinDir\minihoard-mcp.exe"
Write-Host ""
Write-Host "Next: run 'minihoard configure' to set up."
