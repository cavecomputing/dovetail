# Installs the app for the current user on Windows: the release binary in
# %LOCALAPPDATA%\Programs\<app>, a Start Menu entry, and an "Open with" entry for .txt and
# Markdown (.md, .markdown) files.
# Every name comes from src/brand.rs (the binary prints them with --print-brand), so nothing is
# hardcoded here. Run it from a PowerShell window in the repo:
#
#   powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1
#
# Add -Uninstall to remove all of that again (it still builds first, to learn the names); settings
# and scratch stay where they are.
param([switch]$Uninstall)
Set-Location (Split-Path $PSScriptRoot)

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
# Stop at the first error from here on. Not before the build: cargo writes its progress lines to
# stderr, and Windows PowerShell can take those for errors.
$ErrorActionPreference = 'Stop'
# The binary's file name is Cargo.toml's package name.
$package = (Select-String -Path Cargo.toml -Pattern '^name = "(.*)"$' | Select-Object -First 1).Matches[0].Groups[1].Value
$binary = "target\release\$package.exe"
# --print-brand prints shell lines like APP_NAME='...'; each becomes a PowerShell variable. Taking
# its output also makes PowerShell wait for it, even though the release build is a windowed program.
foreach ($line in & $binary --print-brand) {
    if ($line -match "^(\w+)='(.*)'$") {
        Set-Variable -Name $Matches[1] -Value ($Matches[2] -replace "'\\''", "'")
    }
}
# Without names, the paths below would point at the whole Programs folder, which -Uninstall deletes.
if (-not ($APP_NAME -and $BIN_NAME)) { throw "$binary --print-brand printed no names" }

$installDir = "$env:LOCALAPPDATA\Programs\$APP_NAME"
$exe = "$installDir\$BIN_NAME.exe"
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) "$APP_NAME.lnk"
# "Open with": the app's own entry, and its name in the list Windows offers for each file type.
$appKey = "HKCU:\Software\Classes\Applications\$BIN_NAME.exe"
$fileTypes = '.txt', '.md', '.markdown'
$openWithKeys = $fileTypes | ForEach-Object { "HKCU:\Software\Classes\$_\OpenWithList\$BIN_NAME.exe" }

if ($Uninstall) {
    Remove-Item -Recurse -Force (@($installDir, $shortcut, $appKey) + $openWithKeys) -ErrorAction SilentlyContinue
    Write-Output "Removed $APP_NAME. Settings are still in $env:APPDATA\$CONFIG_DIR_NAME and scratch in $env:LOCALAPPDATA\$CONFIG_DIR_NAME"
    exit
}

# Windows won't replace a running .exe, so close the app's windows before reinstalling.
New-Item -ItemType Directory -Force $installDir | Out-Null
Copy-Item $binary $exe -Force
Copy-Item assets\icon\icon.ico "$installDir\icon.ico" -Force

$link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
$link.TargetPath = $exe
$link.IconLocation = "$installDir\icon.ico"
$link.Save()

# The command key first: -Force makes the keys above it, and would empty a key that already exists.
New-Item -Force "$appKey\shell\open\command" -Value "`"$exe`" `"%1`"" | Out-Null
New-ItemProperty -Force $appKey -Name FriendlyAppName -Value $APP_NAME | Out-Null
New-Item -Force "$appKey\SupportedTypes" | Out-Null
foreach ($fileType in $fileTypes) {
    New-ItemProperty -Force "$appKey\SupportedTypes" -Name $fileType -Value '' | Out-Null
}
foreach ($openWithKey in $openWithKeys) {
    New-Item -Force $openWithKey | Out-Null
}

Write-Output "Installed $APP_NAME as $exe"
Write-Output "Settings live in $env:APPDATA\$CONFIG_DIR_NAME and scratch in $env:LOCALAPPDATA\$CONFIG_DIR_NAME"
Write-Output "To open .txt or .md files with it by default: right-click one, Open with, Choose another app."
if (($env:Path -split ';') -notcontains $installDir) {
    Write-Output "Note: to run '$BIN_NAME' from a terminal, add $installDir to your PATH."
}
