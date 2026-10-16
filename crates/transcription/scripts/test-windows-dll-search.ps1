# Release gate for the Windows DLL search order of the transcription bundle
# (p14.1-windows-vulkan#1): the staged pumice-transcribe must load DLLs only
# from its own folder and System32, never from the current directory or PATH.
#
#   pwsh crates/transcription/scripts/test-windows-dll-search.ps1 `
#       -Stage DIR -Model FILE.gguf -Audio FILE.wav [-WorkDir DIR]
#
# Decoy DLLs (windows-dll-decoy.c) named like the bundle's DLLs and like
# vulkan-1.dll, each exporting the real DLL's names so a static import binds
# to it. A loaded decoy records itself in a marker file and ends the process
# with exit code 86. Every case runs twice: decoys only in the current
# directory (PATH unchanged), then decoys only in a directory prepended to
# PATH (the current directory empty). Cases:
#   - complete bundle: backends and file, backend cpu and auto; on a machine
#     without a usable Vulkan device (the CI runners), auto must run on the
#     CPU;
#   - ggml-base.dll missing from a copy of the bundle: the manifest check
#     names it;
#   - ggml-base.dll missing and unlisted in bundle.json, so the manifest
#     check passes and only the loader stands between the program and the
#     decoy: the core DLL preload must fail with the typed installation
#     error (Windows error 126), without loading it.
# Every case runs and failures are reported together, followed by a table of
# every run. Needs the Visual Studio C++ tools (decoys are built with cl).
#
# The test never changes system files. A machine without a Vulkan loader in
# System32, where a planted vulkan-1.dll would be next in the standard search
# order, is checked by the owner on a clean machine or VM
# (p14.1-windows-vulkan#2), not here.
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
$emptyDir = Join-Path $WorkDir 'empty'
$marker = Join-Path $WorkDir 'decoy-loaded.txt'
foreach ($dir in $buildDir, $cwdDir, $pathDir, $emptyDir) {
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
# come from the system loader (only read).
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
$runs = [System.Collections.Generic.List[object]]::new()
$placements = @('cwd', 'path')

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

# Runs the executable with the decoys in one placement: 'cwd' runs it from
# the decoy directory with PATH unchanged, 'path' from an empty directory
# with the decoy directory first on PATH. Records a failure if a decoy
# loaded.
function Invoke-Planted([string]$Case, [string]$Placement, [string]$Exe, [string[]]$Arguments) {
    $Label = "$Case [$Placement]"
    Remove-Item -Path $marker -ErrorAction SilentlyContinue
    $savedPath = $env:PATH
    if ($Placement -eq 'path') {
        $env:PATH = "$pathDir;$savedPath"
        $cwd = $emptyDir
    } else {
        $cwd = $cwdDir
    }
    $env:PUMICE_DECOY_MARKER = $marker
    Push-Location -LiteralPath $cwd
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
    $decoy = if ($loaded) { ($loaded -split "`r?`n") -join ', ' } else { '-' }
    $script:runs.Add([pscustomobject]@{
            Case = $Case; Placement = $Placement; Exit = ('0x{0:X8}' -f $code); Decoy = $decoy
        })
    [pscustomobject]@{ Label = $Label; Code = $code; Out = $out; Envelope = $envelope; Loaded = [bool]$loaded }
}

function Assert-Ok($Run) {
    $Label = $Run.Label
    if ($Run.Loaded) {
        return $false
    }
    if ($Run.Code -ne 0 -or -not $Run.Envelope -or -not $Run.Envelope.ok) {
        $script:failures.Add("${Label}: expected success, got exit $($Run.Code)")
        return $false
    }
    return $true
}

function Assert-Refused($Run, [string[]]$Expect) {
    $Label = $Run.Label
    if ($Run.Loaded) {
        return
    }
    if ($Run.Code -eq 0) {
        $script:failures.Add("${Label}: expected a failure, got exit 0")
        return
    }
    $text = $Run.Out -join "`n"
    foreach ($phrase in $Expect) {
        if (-not $text.Contains($phrase)) {
            $script:failures.Add("${Label}: '$phrase' not reported")
        }
    }
}

$exe = Join-Path $Stage 'pumice-transcribe.exe'
$fileArgs = @('file', $Audio, '--model-file', $Model, '--json')

# 1. Complete bundle.
foreach ($placement in $placements) {
    foreach ($mode in 'cpu', 'auto') {
        $report = Invoke-Planted "complete backends $mode" $placement $exe @('backends', '--backend', $mode, '--json')
        $null = Assert-Ok $report
        $run = Invoke-Planted "complete file $mode" $placement $exe ($fileArgs + @('--backend', $mode))
        if (-not (Assert-Ok $run)) {
            continue
        }
        if (-not $run.Envelope.result.text) {
            $failures.Add("$($run.Label): empty transcript")
        }
        # No usable Vulkan device (the CI runners have none): the GPU module
        # is scanned, registers nothing and auto runs on the CPU.
        $gpu = if ($report.Envelope) { $report.Envelope.result } else { $null }
        if ($mode -eq 'auto' -and $gpu -and $gpu.gpu_module -eq 'scanned' -and $gpu.gpu_devices -eq 0 `
                -and $run.Envelope.result.compute.backend -ne 'cpu') {
            $failures.Add("$($run.Label): no Vulkan device, but backend=$($run.Envelope.result.compute.backend)")
        }
    }
}

# 2. A core DLL missing from a copy of the bundle: the manifest check names it.
$copy = Join-Path $WorkDir 'bundle missing core'
Copy-Item -Recurse -Path $Stage -Destination $copy
Remove-Item -Path (Join-Path $copy 'ggml-base.dll')
$copyExe = Join-Path $copy 'pumice-transcribe.exe'
foreach ($placement in $placements) {
    $run = Invoke-Planted 'missing-core backends cpu' $placement $copyExe @('backends', '--backend', 'cpu', '--json')
    Assert-Refused $run @('ggml-base.dll is listed in bundle.json but missing')
}

# 3. The same, unlisted in bundle.json: only the loader refuses it now, and
# the core DLL preload reports it as an installation error.
$manifestPath = Join-Path $copy 'bundle.json'
$manifest = Get-Content -Raw -Path $manifestPath | ConvertFrom-Json
$manifest.files = @($manifest.files | Where-Object { $_.path -ne 'ggml-base.dll' })
[System.IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 4) + "`n",
    [System.Text.UTF8Encoding]::new($false))
foreach ($placement in $placements) {
    $run = Invoke-Planted 'missing-core-unlisted backends cpu' $placement $copyExe @('backends', '--backend', 'cpu', '--json')
    Assert-Refused $run @('transcribe.dll (Windows error 126; core library unloadable)')
}
Remove-Item -Recurse -Force -Path $copy

$runs | Format-Table -AutoSize -Wrap | Out-String -Width 200 | Write-Host
if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Host "error: $_" }
    throw "DLL search order test failed ($($failures.Count) failures)"
}
Write-Host 'DLL search order ok: no planted decoy was loaded from the current directory or PATH'
# Expected refusals above leave a non-zero native exit status, which a
# calling pwsh step would return as its own.
$global:LASTEXITCODE = 0
