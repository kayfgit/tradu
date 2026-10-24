# Installs (or updates) tradu for the current user and starts it:
#   irm https://raw.githubusercontent.com/kayfgit/tradu/main/install.ps1 | iex
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # the progress bar makes downloads many times slower on PowerShell 5

$url = 'https://github.com/kayfgit/tradu/releases/latest/download/tradu-setup.exe'
$setup = Join-Path $env:TEMP 'tradu-setup.exe'

Write-Host 'Downloading tradu...'
Invoke-WebRequest $url -OutFile $setup -UseBasicParsing

# An update can't replace the exe while it's running.
Get-Process tradu -ErrorAction SilentlyContinue | Stop-Process -Force

Write-Host 'Installing...'
Start-Process $setup -ArgumentList '/S' -Wait
Remove-Item $setup -ErrorAction SilentlyContinue

$exe = Join-Path $env:LOCALAPPDATA 'tradu\tradu.exe'
if (-not (Test-Path $exe)) { throw "Install finished but $exe is missing" }
Start-Process $exe
Write-Host 'tradu is running in the tray. Press Ctrl+Shift+Space anywhere to translate.'
