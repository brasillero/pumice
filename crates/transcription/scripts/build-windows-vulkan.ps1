# Build pumice-transcribe with runtime CPU-variant selection plus the optional
# Vulkan module and stage a self-contained bundle. x86-64 Windows (MSVC) only;
# experimental (P14), a CI artifact, not a release.
#
#   pwsh crates/transcription/scripts/build-windows-vulkan.ps1 [-TargetDir DIR]
#
# Windows counterpart of build-cpu-variants.sh, same design: its own
# CARGO_TARGET_DIR (CMake never reuses another build's cache), an explicit
# TRANSCRIBE_CMAKE_ARGS (wins over the portable baseline in .cargo/config.toml,
# force=false), and staging ONLY from the transcribe-cpp-sys out_dir of the
# build just run. The result lands in <TargetDir>/stage/:
#
#   pumice-transcribe.exe
#   transcribe.dll, ggml.dll, ggml-base.dll       core DLLs beside the exe
#   cpu/<variant>/ggml-cpu-<variant>.dll          one CPU module per directory
#   gpu/vulkan/ggml-vulkan.dll                    the only Vulkan importer
#
# Windows resolves the core DLLs from the executable's directory; startup
# points ggml at exactly one cpu/<variant>/ directory and then gpu/vulkan/.
# No .lib/.pdb/CMake/SDK files are staged. Nothing is written at runtime.
#
# Static CRT (p14.1-windows-vulkan/D6): Rust uses +crt-static from
# .cargo/config.toml and the native build gets CMAKE_MSVC_RUNTIME_LIBRARY=
# MultiThreaded (/MT). The audit at the end fails on any import of a VC++ or
# UCRT redistributable DLL, and on vulkan-1.dll anywhere but the GPU module.
# Needs the Vulkan SDK (VULKAN_SDK, glslc on PATH) and Visual Studio 2022 C++
# tools at build time only.
[CmdletBinding()]
param(
    [string]$TargetDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
if (-not $TargetDir) {
    $TargetDir = Join-Path $repoRoot 'target\windows-vulkan'
}
$triple = 'x86_64-pc-windows-msvc'
$stageDir = Join-Path $TargetDir 'stage'
New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null

# The MSVC variant set (engine.rs MSVC_BUILT): ggml's x86 table minus the
# entries it skips under MSVC. A different set means the engine changed and
# the Rust chooser must be re-audited.
$expectedVariants = @(
    'alderlake', 'cannonlake', 'cascadelake', 'haswell', 'icelake',
    'sandybridge', 'skylakex', 'sse42', 'x64'
)
$coreDlls = @('transcribe.dll', 'ggml.dll', 'ggml-base.dll')
$gpuDll = 'ggml-vulkan.dll'

$env:CARGO_TARGET_DIR = $TargetDir
$env:TRANSCRIBE_CMAKE_ARGS = '-DGGML_NATIVE=OFF -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF ' +
    '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded'
# Ambient settings that would change or bypass the native build.
foreach ($name in 'CMAKE_ARGS', 'TRANSCRIBE_DIR', 'GGML_BACKEND_PATH') {
    Remove-Item -Path "Env:$name" -ErrorAction SilentlyContinue
}

Write-Host "building pumice-transcribe (vulkan, release, $triple) in $TargetDir"
$jsonLog = Join-Path $TargetDir 'cargo-build.json'
& cargo build --release --locked -p pumice-transcription --features vulkan `
    --target $triple --message-format=json | Out-File -FilePath $jsonLog -Encoding utf8
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
}

$messages = Get-Content -Path $jsonLog | Where-Object { $_.StartsWith('{') } |
    ForEach-Object { $_ | ConvertFrom-Json }
# The exact out_dir of this build: several cached transcribe-cpp-sys-<hash>
# directories can coexist, and the wrong one pairs the fresh executable with
# stale native libraries.
$sysRun = $messages | Where-Object {
    $_.reason -eq 'build-script-executed' -and $_.package_id -like '*transcribe-cpp-sys*'
} | Select-Object -Last 1
if (-not $sysRun) {
    throw 'no transcribe-cpp-sys build-script output in the current build'
}
$sysOut = $sysRun.out_dir
$exeArtifact = $messages | Where-Object {
    $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'pumice-transcribe' -and
    $_.PSObject.Properties.Name -contains 'executable' -and $_.executable
} | Select-Object -Last 1
if (-not $exeArtifact) {
    throw 'no pumice-transcribe executable in the current build'
}
$exe = $exeArtifact.executable
Write-Host "staging native libraries from $sysOut"

$cmakeCache = Join-Path $sysOut 'build\CMakeCache.txt'
if (Test-Path $cmakeCache) {
    Write-Host 'native build settings (CMakeCache.txt):'
    Select-String -Path $cmakeCache -Pattern '^(CMAKE_MSVC_RUNTIME_LIBRARY|GGML_VULKAN|GGML_BACKEND_DL|GGML_CPU_ALL_VARIANTS|GGML_NATIVE|CMAKE_GENERATOR):' |
        ForEach-Object { Write-Host "  $($_.Line)" }
}

# Windows installs runtime DLLs and modules under out/bin (import libraries
# and manifests under out/lib). Classify every DLL there; an unknown one
# means the native build changed and is not staged silently.
$binDir = Join-Path $sysOut 'bin'
$dlls = @(Get-ChildItem -Path $binDir -Filter '*.dll' -File)
Write-Host "native runtime files in ${binDir}:"
Get-ChildItem -Path $binDir -File | ForEach-Object { Write-Host "  $($_.Name) ($($_.Length) bytes)" }
$cpuModules = @($dlls | Where-Object { $_.Name -match '^ggml-cpu-(.+)\.dll$' })
$unknown = @($dlls | Where-Object {
    $coreDlls -notcontains $_.Name -and $_.Name -ne $gpuDll -and $_.Name -notmatch '^ggml-cpu-.+\.dll$'
})
if ($unknown.Count -gt 0) {
    throw "unexpected DLLs in ${binDir}: $($unknown.Name -join ', ')"
}
foreach ($name in $coreDlls + $gpuDll) {
    if (-not (Test-Path (Join-Path $binDir $name) -PathType Leaf)) {
        throw "expected library $name missing from $binDir"
    }
}
$variants = @($cpuModules | ForEach-Object { $_.Name -replace '^ggml-cpu-(.+)\.dll$', '$1' } | Sort-Object)
if (($variants -join ',') -ne ($expectedVariants -join ',')) {
    throw "CPU variant set changed: built [$($variants -join ', ')], expected [$($expectedVariants -join ', ')]"
}

if (Test-Path $stageDir) {
    Remove-Item -Recurse -Force -Path $stageDir
}
New-Item -ItemType Directory -Force -Path $stageDir | Out-Null
Copy-Item -Path $exe -Destination (Join-Path $stageDir 'pumice-transcribe.exe')
foreach ($name in $coreDlls) {
    Copy-Item -Path (Join-Path $binDir $name) -Destination $stageDir
}
# One CPU module per variant directory: ggml's directory probe scores
# whatever it finds, so each directory holds exactly its own module.
foreach ($module in $cpuModules) {
    $variant = $module.Name -replace '^ggml-cpu-(.+)\.dll$', '$1'
    $dir = Join-Path $stageDir "cpu\$variant"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Copy-Item -Path $module.FullName -Destination $dir
}
$gpuDir = Join-Path $stageDir 'gpu\vulkan'
New-Item -ItemType Directory -Force -Path $gpuDir | Out-Null
Copy-Item -Path (Join-Path $binDir $gpuDll) -Destination $gpuDir

# Staged layout and sizes.
$files = @(Get-ChildItem -Path $stageDir -Recurse -File | Sort-Object FullName)
$total = 0
Write-Host "staged ${stageDir}:"
foreach ($file in $files) {
    $relative = $file.FullName.Substring($stageDir.Length + 1)
    $total += $file.Length
    Write-Host ('  {0,12:N0}  {1}' -f $file.Length, $relative)
}
Write-Host ('  {0,12:N0}  total ({1} files)' -f $total, $files.Count)

# Dependency audit with dumpbin from the Visual Studio C++ tools.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$dumpbin = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe' | Select-Object -First 1
if (-not $dumpbin) {
    throw 'dumpbin.exe not found (Visual Studio C++ x64 tools)'
}
# Redistributable runtimes an end user would have to install.
$forbidden = '^(vcruntime|msvcp|api-ms-win-crt-|ucrtbase|concrt|vcomp|vccorlib)'
$bundled = @($coreDlls)
$system32 = Join-Path $env:SystemRoot 'System32'
$failures = @()
Write-Host 'dependency audit (dumpbin /dependents):'
foreach ($file in $files) {
    $relative = $file.FullName.Substring($stageDir.Length + 1)
    $output = & $dumpbin /nologo /dependents $file.FullName
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin failed on $relative"
    }
    $imports = @($output | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '^\S+\.dll$' })
    Write-Host "  ${relative}: $($imports -join ', ')"
    foreach ($import in $imports) {
        $lower = $import.ToLowerInvariant()
        if ($lower -match $forbidden) {
            $failures += "$relative imports redistributable runtime $import"
        } elseif ($lower -eq 'vulkan-1.dll') {
            if ($relative -ne "gpu\vulkan\$gpuDll") {
                $failures += "$relative imports vulkan-1.dll (only gpu\vulkan\$gpuDll may)"
            }
        } elseif ($bundled -contains $lower) {
            # Resolved from the executable's directory.
        } elseif (-not (Test-Path (Join-Path $system32 $import) -PathType Leaf)) {
            $failures += "$relative imports $import, which is neither bundled nor an OS DLL"
        }
    }
}
if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Host "error: $_" }
    throw 'dependency audit failed'
}
Write-Host 'dependency audit ok: static CRT, only the GPU module imports vulkan-1.dll'
