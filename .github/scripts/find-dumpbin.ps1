# Locate dumpbin.exe from an installed Visual Studio and append its directory
# to GITHUB_PATH so later steps can run `dumpbin`. Replaces ilammy/msvc-dev-cmd,
# whose latest release still declares Node 20.
#
# Run by Windows CI jobs only (paths are not exercised on Linux or macOS).
$ErrorActionPreference = 'Stop'

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
    throw "vswhere not found at '$vswhere'"
}

$vsInstall = (& $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath | Select-Object -First 1)
if (-not $vsInstall) {
    throw 'No Visual Studio installation with MSVC tools found'
}
$vsInstall = $vsInstall.Trim()

# Native toolset layout: <VS>\VC\Tools\MSVC\<version>\bin\Hostx64\x64\dumpbin.exe
$msvcRoot = Join-Path $vsInstall 'VC\Tools\MSVC'
$dumpbin = Get-ChildItem -LiteralPath $msvcRoot -Directory |
    Sort-Object -Property Name -Descending |
    ForEach-Object { Join-Path $_.FullName 'bin\Hostx64\x64\dumpbin.exe' } |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
    Select-Object -First 1
if (-not $dumpbin) {
    throw "dumpbin.exe not found under '$msvcRoot'"
}

$dumpbinDir = Split-Path -Parent $dumpbin
Add-Content -LiteralPath $env:GITHUB_PATH -Value $dumpbinDir
Write-Host "dumpbin: $dumpbin"
