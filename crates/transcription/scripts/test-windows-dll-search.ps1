# Release gate for the Windows DLL search order of the transcription bundle
# (p14.1-windows-vulkan#1): the staged pumice-transcribe must load DLLs only
# from its own folder and System32, never from the current directory or PATH.
#
#   pwsh crates/transcription/scripts/test-windows-dll-search.ps1 `
#       -Stage DIR -Model FILE.gguf -Audio FILE.wav [-WorkDir DIR]
#
# Decoy DLLs (windows-dll-decoy.c) named like the bundle's DLLs and like
# vulkan-1.dll, each exporting the real DLL's names so a static import binds
# to it, are planted in the current directory and in a directory at the
# front of PATH. A loaded decoy records itself in a marker file and ends the
# process with exit code 86. Cases:
#   - complete bundle: backends and file, backend cpu and auto;
#   - System32\vulkan-1.dll hidden (a machine without a Vulkan driver), so
#     the planted vulkan-1.dll would be next in the standard search order:
#     backend auto must run on the CPU;
#   - ggml-base.dll missing from a copy of the bundle: an installation error;
#   - ggml-base.dll missing and unlisted in bundle.json, so the manifest
#     check passes and only the loader stands between the program and the
#     decoy: it must fail without loading it.
# Every case runs and failures are reported together. Needs the Visual
# Studio C++ tools (decoys are built with cl) and administrator rights for
# the hidden-loader case (it renames System32\vulkan-1.dll and restores it).
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Stage,
    [Parameter(Mandatory)][string]$Model,
    [Parameter(Mandatory)][string]$Audio,
    [string]$WorkDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$sentinel = 86
$decoyNames = @('transcribe.dll', 'ggml.dll', 'ggml-base.dll', 'ggml-vulkan.dll', 'vulkan-1.dll')
$systemVulkan = Join-Path $env:SystemRoot 'System32\vulkan-1.dll'

$Stage = (Resolve-Path -LiteralPath $Stage).Path
$Model = (Resolve-Path -LiteralPath $Model).Path
$Audio = (Resolve-Path -LiteralPath $Audio).Path
if (-not $WorkDir) {
    $WorkDir = Join-Path ([System.IO.Path]::GetTempPath()) 'pumice-dll-search'
}
if (Test-Path $WorkDir) {
    Remove-Item -Recurse -Force -Path $WorkDir
}
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
$WorkDir = (Resolve-Path -LiteralPath $WorkDir).Path
$buildDir = Join-Path $WorkDir 'build'
$cwdDir = Join-Path $WorkDir 'cwd'
$pathDir = Join-Path $WorkDir 'path'
$marker = Join-Path $WorkDir 'decoy-loaded.txt'
foreach ($dir in $buildDir, $cwdDir, $pathDir) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath | Select-Object -First 1
if (-not $vsPath) {
    throw 'Visual Studio C++ x64 tools not found'
}
$vcvars = Join-Path $vsPath 'VC\Auxiliary\Build\vcvars64.bat'
$dumpbin = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe' | Select-Object -First 1
if (-not (Test-Path $vcvars) -or -not $dumpbin) {
    throw 'vcvars64.bat or dumpbin.exe not found (Visual Studio C++ x64 tools)'
}

# The exported names of a real DLL, from dumpbin's table (forwarders
# included, ordinal-only entries skipped).
function Get-ExportNames([string]$Path) {
    $output = & $dumpbin /nologo /exports $Path
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin /exports failed on $Path"
    }
    $inTable = $false
    $names = foreach ($line in $output) {
        if ($line -match '^\s+ordinal\s+hint\s+RVA\s+name') {
            $inTable = $true
        } elseif ($inTable -and $line -match '^\s*Summary') {
            break
        } elseif ($inTable -and $line -match '^\s+\d+\s+[0-9A-Fa-f]+\s+(?:[0-9A-Fa-f]{8}\s+)?([^\s\[\(]+)') {
            $Matches[1]
        }
    }
    @($names)
}

# Each decoy exports the names of the DLL it impersonates; vulkan-1.dll's
# come from the system loader, read before any case hides it.
$sources = @{
    'transcribe.dll'  = Join-Path $Stage 'transcribe.dll'
    'ggml.dll'        = Join-Path $Stage 'ggml.dll'
    'ggml-base.dll'   = Join-Path $Stage 'ggml-base.dll'
    'ggml-vulkan.dll' = Join-Path $Stage 'gpu\vulkan\ggml-vulkan.dll'
    'vulkan-1.dll'    = $systemVulkan
}
$decoySource = Join-Path $PSScriptRoot 'windows-dll-decoy.c'
$buildLines = @('@echo off', "call `"$vcvars`" >nul || exit /b 1")
foreach ($name in $decoyNames) {
    $base = [System.IO.Path]::GetFileNameWithoutExtension($name)
    $exports = @(if (Test-Path $sources[$name]) { Get-ExportNames $sources[$name] })
    if ($name -eq 'vulkan-1.dll' -and $exports.Count -eq 0) {
        # No system loader to copy from: the entry points ggml-vulkan.dll
        # imports, which is what a static import must find.
        $exports = @('vkCreateInstance', 'vkEnumerateInstanceVersion',
            'vkEnumerateInstanceExtensionProperties', 'vkEnumerateInstanceLayerProperties',
            'vkGetInstanceProcAddr', 'vkGetDeviceProcAddr', 'vkDestroyInstance')
    }
    Write-Host "decoy $name exports $($exports.Count) names"
    $def = Join-Path $buildDir "$base.def"
    $defLines = @("LIBRARY `"$name`"", 'EXPORTS') + @($exports | ForEach-Object { "    $_=decoy_stub" })
    Set-Content -Path $def -Value $defLines -Encoding ascii
    $out = Join-Path $buildDir $name
    $buildLines += "cl /nologo /O1 /GS- /LD `"$decoySource`" /Fo`"$buildDir\$base.obj`" " +
        "/link /NODEFAULTLIB /ENTRY:DllMain kernel32.lib /DEF:`"$def`" /OUT:`"$out`" " +
        "/IMPLIB:`"$buildDir\$base.lib`" >`"$buildDir\$base.log`" || (type `"$buildDir\$base.log`" & exit /b 1)"
}
$buildScript = Join-Path $buildDir 'build-decoys.cmd'
Set-Content -Path $buildScript -Value $buildLines -Encoding ascii
& cmd.exe /d /c $buildScript
if ($LASTEXITCODE -ne 0) {
    throw "building the decoy DLLs failed with exit code $LASTEXITCODE"
}
foreach ($name in $decoyNames) {
    Copy-Item -Path (Join-Path $buildDir $name) -Destination $cwdDir
    Copy-Item -Path (Join-Path $buildDir $name) -Destination $pathDir
}

$failures = [System.Collections.Generic.List[string]]::new()

# Positive control: each decoy, loaded on purpose in a child process, is
# detected (marker written, sentinel exit code).
foreach ($name in $decoyNames) {
    Remove-Item -Path $marker -ErrorAction SilentlyContinue
    $env:PUMICE_DECOY_MARKER = $marker
    $path = Join-Path $buildDir $name
    & pwsh -NoProfile -NonInteractive -Command "[System.Runtime.InteropServices.NativeLibrary]::Load('$path') | Out-Null"
    $code = $LASTEXITCODE
    Remove-Item Env:PUMICE_DECOY_MARKER
    if ($code -ne $sentinel -or -not (Test-Path $marker)) {
        $failures.Add("control ${name}: a deliberate load was not detected (exit $code)")
    }
}
Remove-Item -Path $marker -ErrorAction SilentlyContinue
Write-Host "positive control: $($decoyNames.Count) decoys checked"

# Runs the executable from the decoy directory with the other decoy
# directory first on PATH; records a failure if a decoy loaded.
function Invoke-Planted([string]$Label, [string]$Exe, [string[]]$Arguments) {
    Remove-Item -Path $marker -ErrorAction SilentlyContinue
    $savedPath = $env:PATH
    $env:PATH = "$pathDir;$savedPath"
    $env:PUMICE_DECOY_MARKER = $marker
    Push-Location -LiteralPath $cwdDir
    try {
        # Native stderr (the loader's status lines) is expected output.
        $ErrorActionPreference = 'Continue'
        $out = @(& $Exe @Arguments 2>&1 | ForEach-Object { "$_" })
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = 'Stop'
        Pop-Location
        $env:PATH = $savedPath
        Remove-Item Env:PUMICE_DECOY_MARKER
    }
    $out | ForEach-Object { Write-Host "  [$Label] $_" }
    Write-Host ("  [$Label] exit {0} (0x{0:X8})" -f $code)
    $loaded = if (Test-Path $marker) { (Get-Content -Raw -Encoding Unicode -Path $marker).Trim() } else { '' }
    if ($loaded -or $code -eq $sentinel) {
        $script:failures.Add("${Label}: a planted decoy was loaded (exit $code): $loaded")
    }
    $json = $out | Where-Object { $_.StartsWith('{') } | Select-Object -Last 1
    $envelope = if ($json) { $json | ConvertFrom-Json } else { $null }
    [pscustomobject]@{ Code = $code; Out = $out; Envelope = $envelope; Loaded = [bool]$loaded }
}

function Assert-Ok([string]$Label, $Run) {
    if ($Run.Loaded) {
        return $false
    }
    if ($Run.Code -ne 0 -or -not $Run.Envelope -or -not $Run.Envelope.ok) {
        $script:failures.Add("${Label}: expected success, got exit $($Run.Code)")
        return $false
    }
    return $true
}

function Assert-Refused([string]$Label, $Run, [string]$Expect) {
    if ($Run.Loaded) {
        return
    }
    if ($Run.Code -eq 0) {
        $script:failures.Add("${Label}: expected a failure, got exit 0")
    } elseif ($Expect -and -not (($Run.Out -join "`n").Contains($Expect))) {
        $script:failures.Add("${Label}: '$Expect' not reported")
    }
}

$exe = Join-Path $Stage 'pumice-transcribe.exe'
$fileArgs = @('file', $Audio, '--model-file', $Model, '--json')

# 1. Complete bundle.
foreach ($mode in 'cpu', 'auto') {
    $run = Invoke-Planted "complete backends $mode" $exe @('backends', '--backend', $mode, '--json')
    $null = Assert-Ok "complete backends $mode" $run
    $run = Invoke-Planted "complete file $mode" $exe ($fileArgs + @('--backend', $mode))
    if ((Assert-Ok "complete file $mode" $run) -and -not $run.Envelope.result.text) {
        $failures.Add("complete file ${mode}: empty transcript")
    }
}

# 2. No Vulkan loader in System32.
$hidden = "$systemVulkan.pumice-hidden"
$wasPresent = Test-Path $systemVulkan
try {
    if ($wasPresent) {
        Rename-Item -LiteralPath $systemVulkan -NewName (Split-Path -Leaf $hidden)
    }
    Write-Host "System32\vulkan-1.dll hidden (was present: $wasPresent)"
    $run = Invoke-Planted 'no-loader backends auto' $exe @('backends', '--backend', 'auto', '--json')
    if (Assert-Ok 'no-loader backends auto' $run) {
        $result = $run.Envelope.result
        if ($result.gpu_module -ne 'scanned' -or $result.gpu_devices -ne 0) {
            $failures.Add("no-loader backends auto: gpu_module=$($result.gpu_module) gpu_devices=$($result.gpu_devices)")
        }
    }
    $run = Invoke-Planted 'no-loader file auto' $exe ($fileArgs + @('--backend', 'auto'))
    if ((Assert-Ok 'no-loader file auto' $run) -and $run.Envelope.result.compute.backend -ne 'cpu') {
        $failures.Add("no-loader file auto: backend=$($run.Envelope.result.compute.backend)")
    }
} finally {
    if ($wasPresent -and (Test-Path $hidden)) {
        Rename-Item -LiteralPath $hidden -NewName (Split-Path -Leaf $systemVulkan)
    }
}

# 3. A core DLL missing from a copy of the bundle: the manifest check names it.
$copy = Join-Path $WorkDir 'bundle missing core'
Copy-Item -Recurse -Path $Stage -Destination $copy
Remove-Item -Path (Join-Path $copy 'ggml-base.dll')
$copyExe = Join-Path $copy 'pumice-transcribe.exe'
$run = Invoke-Planted 'missing-core backends cpu' $copyExe @('backends', '--backend', 'cpu', '--json')
Assert-Refused 'missing-core backends cpu' $run 'ggml-base.dll is listed in bundle.json but missing'

# 4. The same, unlisted in bundle.json: only the loader refuses it now.
$manifestPath = Join-Path $copy 'bundle.json'
$manifest = Get-Content -Raw -Path $manifestPath | ConvertFrom-Json
$manifest.files = @($manifest.files | Where-Object { $_.path -ne 'ggml-base.dll' })
[System.IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 4) + "`n",
    [System.Text.UTF8Encoding]::new($false))
$run = Invoke-Planted 'missing-core-unlisted backends cpu' $copyExe @('backends', '--backend', 'cpu', '--json')
Assert-Refused 'missing-core-unlisted backends cpu' $run $null
Remove-Item -Recurse -Force -Path $copy

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Host "error: $_" }
    throw "DLL search order test failed ($($failures.Count) failures)"
}
Write-Host 'DLL search order ok: no planted decoy was loaded from the current directory or PATH'
# Expected refusals above leave a non-zero native exit status, which a
# calling pwsh step would return as its own.
$global:LASTEXITCODE = 0
