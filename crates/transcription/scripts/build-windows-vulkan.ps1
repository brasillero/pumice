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
#   bundle.json                                   manifest: sizes and SHA-256
#
# The executable delay-loads transcribe.dll (build.rs) and its main first
# limits the DLL search to its own directory and System32 (src/main.rs), so
# the core DLLs, the modules ggml loads by full path and their dependencies
# (vulkan-1.dll included) never come from the current directory or PATH.
# Startup points ggml at exactly one cpu/<variant>/ directory and then
# gpu/vulkan/. No .lib/.pdb/CMake/SDK files are staged. Nothing is written
# at runtime.
#
# bundle.json (src/bundle.rs) lists every staged file with its role, size and
# SHA-256, plus the product version, target and engine. On Windows startup
# requires it and checks the core DLLs, the selected CPU module and the GPU
# module against it before registering any of them, so a damaged or
# mixed-version bundle fails instead of running without its GPU module.
#
# Static CRT (p14.1-windows-vulkan/D6): Rust uses +crt-static from
# .cargo/config.toml and the native build gets CMAKE_MSVC_RUNTIME_LIBRARY=
# MultiThreaded (/MT). The audit at the end accepts only reviewed imports
# (Get-ImportFailures): a short list of OS DLLs, the bundled core DLLs, and
# vulkan-1.dll for the GPU module alone; anything else fails the build, as
# does an engine DLL imported directly (not delay-loaded) by the executable.
# Its self-test runs first (-SelfTestOnly runs just that, without a build).
# Needs the Vulkan SDK (VULKAN_SDK, glslc on PATH) and Visual Studio 2022 C++
# tools at build time only.
[CmdletBinding()]
param(
    [string]$TargetDir,
    [switch]$SelfTestOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$coreDlls = @('transcribe.dll', 'ggml.dll', 'ggml-base.dll')
$gpuDll = 'ggml-vulkan.dll'
$gpuModule = "gpu\vulkan\$gpuDll"

# The OS imports of this bundle, reviewed from dumpbin /dependents on
# windows-2022 (P14.1): Rust's std (kernel32, ntdll, ws2_32, bcrypt,
# bcryptprimitives, advapi32 and the WaitOnAddress API set) and the /MT
# native libraries (kernel32, advapi32). Every Windows 10+ x64 installation
# provides them. A new import fails the audit until it is reviewed and
# added here; being present in System32 on the build machine proves nothing
# (older CRTs such as MSVCR120.dll often are).
$systemImports = @(
    'kernel32.dll', 'ntdll.dll', 'advapi32.dll', 'ws2_32.dll', 'bcrypt.dll',
    'bcryptprimitives.dll', 'api-ms-win-core-synch-l1-2-0.dll'
)
# Redistributable runtimes an end user would have to install; named in the
# failure so the cause is obvious.
$redistributable = '^(vcruntime|msvcp|msvcr|api-ms-win-crt-|ucrtbase|concrt|vcomp|vccorlib)'

# The failures for one staged file's imports and delay-load imports (file
# relative to the bundle root, Windows separators); nothing when all are
# allowed.
function Get-ImportFailures([string]$Relative, [string[]]$Imports, [string[]]$DelayImports = @()) {
    if ($Imports.Count -eq 0) {
        # Every PE imports kernel32: an empty list means dumpbin's output
        # was not parsed.
        "$Relative has no parsed imports"
        return
    }
    if ($Relative -eq 'pumice-transcribe.exe') {
        # A static import is resolved before main, with the standard search
        # order (current directory, PATH); a delay-loaded one only after main
        # has restricted the search.
        foreach ($import in $Imports) {
            if ($coreDlls -contains $import.ToLowerInvariant()) {
                "$Relative imports $import directly (engine DLLs must be delay-loaded)"
            }
        }
    }
    foreach ($import in @(@($Imports) + @($DelayImports) | Where-Object { $_ })) {
        $lower = $import.ToLowerInvariant()
        if ($systemImports -contains $lower -or $coreDlls -contains $lower) {
            continue
        }
        if ($lower -eq 'vulkan-1.dll' -and $Relative -eq $gpuModule) {
            continue
        }
        if ($lower -match $redistributable) {
            "$Relative imports redistributable runtime $import"
        } elseif ($lower -eq 'vulkan-1.dll') {
            "$Relative imports vulkan-1.dll (only $gpuModule may)"
        } else {
            "$Relative imports $import, which is not on the reviewed allowlist"
        }
    }
}

# Synthetic import lists with known verdicts, checked before anything is
# built or audited.
function Test-ImportAudit {
    $exeImports = @(
        'ws2_32.dll', 'api-ms-win-core-synch-l1-2-0.dll', 'kernel32.dll', 'ntdll.dll',
        'bcryptprimitives.dll', 'bcrypt.dll', 'advapi32.dll'
    )
    $cases = @(
        @{ file = 'pumice-transcribe.exe'; imports = $exeImports; delay = @('transcribe.dll'); expect = @() },
        @{ file = 'pumice-transcribe.exe'; imports = $exeImports + 'transcribe.dll'; expect = @('transcribe.dll directly') },
        @{ file = 'pumice-transcribe.exe'; imports = $exeImports; delay = @('transcribe.dll', 'MSVCP140.dll')
           expect = @('redistributable runtime MSVCP140.dll') },
        @{ file = $gpuModule; imports = @('ggml-base.dll', 'vulkan-1.dll', 'KERNEL32.dll'); expect = @() },
        @{ file = 'cpu\x64\ggml-cpu-x64.dll'; imports = @('ggml-base.dll', 'KERNEL32.dll', 'ADVAPI32.dll'); expect = @() },
        @{ file = 'transcribe.dll'; imports = @('ggml.dll', 'MSVCR120.dll'); expect = @('redistributable runtime MSVCR120.dll') },
        @{ file = 'ggml.dll'; imports = @('VCRUNTIME140.dll', 'MSVCP140.dll', 'ucrtbase.dll', 'api-ms-win-crt-runtime-l1-1-0.dll')
           expect = @('redistributable runtime VCRUNTIME140.dll', 'redistributable runtime MSVCP140.dll',
                      'redistributable runtime ucrtbase.dll', 'redistributable runtime api-ms-win-crt-runtime-l1-1-0.dll') },
        @{ file = 'ggml-base.dll'; imports = @('KERNEL32.dll', 'unknown.dll', 'api-ms-win-core-file-l1-1-0.dll', 'user32.dll')
           expect = @('unknown.dll, which is not on', 'api-ms-win-core-file-l1-1-0.dll, which is not on', 'user32.dll, which is not on') },
        @{ file = 'cpu\x64\ggml-cpu-x64.dll'; imports = @('KERNEL32.dll', 'vulkan-1.dll'); expect = @('vulkan-1.dll (only') },
        @{ file = 'ggml-vulkan.dll'; imports = @('vulkan-1.dll', 'KERNEL32.dll'); expect = @('vulkan-1.dll (only') },
        @{ file = 'pumice-transcribe.exe'; imports = @(); expect = @('no parsed imports') }
    )
    $errors = @()
    foreach ($case in $cases) {
        $delay = @(if ($case.ContainsKey('delay')) { $case.delay })
        $got = @(Get-ImportFailures $case.file $case.imports $delay)
        $ok = $got.Count -eq $case.expect.Count
        for ($i = 0; $ok -and $i -lt $got.Count; $i++) {
            $ok = $got[$i].Contains($case.expect[$i])
        }
        if (-not $ok) {
            $errors += "$($case.file) [$($case.imports -join ', ')]: got [$($got -join '; ')]"
        }
    }
    if ($errors.Count -gt 0) {
        $errors | ForEach-Object { Write-Host "error: audit self-test: $_" }
        throw 'dependency audit self-test failed'
    }
    Write-Host "dependency audit self-test ok ($($cases.Count) cases)"
}

Test-ImportAudit
if ($SelfTestOnly) {
    return
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
if (-not $TargetDir) {
    $TargetDir = Join-Path $repoRoot 'target\windows-vulkan'
}
$triple = 'x86_64-pc-windows-msvc'
New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null
# Absolute from here on: staged files are named by cutting this prefix off
# their full paths, and cargo resolves a relative CARGO_TARGET_DIR elsewhere.
$TargetDir = (Resolve-Path -LiteralPath $TargetDir).Path
$stageDir = Join-Path $TargetDir 'stage'

# The MSVC variant set (engine.rs MSVC_BUILT): ggml's x86 table minus the
# entries it skips under MSVC. A different set means the engine changed and
# the Rust chooser must be re-audited.
$expectedVariants = @(
    'alderlake', 'cannonlake', 'cascadelake', 'haswell', 'icelake',
    'sandybridge', 'skylakex', 'sse42', 'x64'
)
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

# bundle.json: identity from cargo metadata (the versions this lockfile
# builds with the same features and target; without the feature the engine
# is not in the resolve), then every staged file. Written without a BOM,
# which serde_json would reject.
$metadata = & cargo metadata --format-version 1 --locked --filter-platform $triple `
    --features pumice-transcription/vulkan | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with exit code $LASTEXITCODE"
}
function Get-PackageVersion([string]$Name) {
    $found = @($metadata.packages | Where-Object { $_.name -eq $Name })
    if ($found.Count -ne 1) {
        throw "cargo metadata lists $($found.Count) $Name packages, expected 1"
    }
    return $found[0].version
}
$productVersion = Get-PackageVersion 'pumice-transcription'
$engineVersion = Get-PackageVersion 'transcribe-cpp-sys'
$entries = foreach ($file in $files) {
    $relative = $file.FullName.Substring($stageDir.Length + 1).Replace('\', '/')
    $role = if ($relative -eq 'pumice-transcribe.exe') {
        'executable'
    } elseif ($coreDlls -contains $relative) {
        'core'
    } elseif ($relative -like 'cpu/*') {
        'cpu'
    } elseif ($relative -like 'gpu/*') {
        'gpu'
    } else {
        throw "unclassified staged file $relative"
    }
    [ordered]@{
        path = $relative
        role = $role
        size = $file.Length
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash.ToLowerInvariant()
    }
}
$manifest = [ordered]@{
    schema_version = 1
    product = 'pumice-transcribe'
    version = $productVersion
    target = $triple
    features = @('vulkan')
    engine = "transcribe-cpp-sys $engineVersion"
    files = @($entries)
}
$manifestPath = Join-Path $stageDir 'bundle.json'
[System.IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 4) + "`n",
    [System.Text.UTF8Encoding]::new($false))
Write-Host "wrote bundle.json: pumice-transcribe $productVersion ($triple), transcribe-cpp-sys $engineVersion, $($files.Count) files"

# Dependency audit with dumpbin from the Visual Studio C++ tools, against the
# reviewed allowlist above.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$dumpbin = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe' | Select-Object -First 1
if (-not $dumpbin) {
    throw 'dumpbin.exe not found (Visual Studio C++ x64 tools)'
}
$failures = @()
Write-Host 'dependency audit (dumpbin /dependents):'
foreach ($file in $files) {
    $relative = $file.FullName.Substring($stageDir.Length + 1)
    $output = & $dumpbin /nologo /dependents $file.FullName
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin failed on $relative"
    }
    # dumpbin lists static imports, then delay-load imports under their own
    # heading.
    $imports = @()
    $delayImports = @()
    $inDelay = $false
    foreach ($line in $output) {
        $trimmed = "$line".Trim()
        if ($trimmed -match 'delay load dependencies') {
            $inDelay = $true
        } elseif ($trimmed -match '^\S+\.dll$') {
            if ($inDelay) { $delayImports += $trimmed } else { $imports += $trimmed }
        }
    }
    $delayNote = if ($delayImports.Count -gt 0) { " (delay-loaded: $($delayImports -join ', '))" } else { '' }
    Write-Host "  ${relative}: $($imports -join ', ')$delayNote"
    $failures += @(Get-ImportFailures $relative $imports $delayImports)
}
if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Host "error: $_" }
    throw 'dependency audit failed'
}
Write-Host 'dependency audit ok: static CRT, only reviewed OS imports, engine DLLs delay-loaded by the executable, only the GPU module imports vulkan-1.dll'
