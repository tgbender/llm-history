$ErrorActionPreference = 'Stop'

$lockfile = Join-Path $PSScriptRoot '..\portablemsvc.lock'
$lock = Get-Content -LiteralPath $lockfile -Raw | ConvertFrom-Json
if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    throw 'The locked PortableMSVC build currently supports x64 Windows.'
}

$kit = & portablemsvc get-path --lockfile $lockfile
if ($LASTEXITCODE -ne 0 -or -not $kit) {
    throw 'The locked MSVC kit is missing. Run mise run setup:windows first.'
}

$spec = Get-Content -LiteralPath (Join-Path $kit 'env.json') -Raw | ConvertFrom-Json
if ($spec.VCToolsVersion -ne $lock.resolved.msvc.vctools_version -or
    $spec.PORTABLE_SDK_VERSION -ne $lock.resolved.sdk.version -or
    ($spec.VSCMD_ARG_TGT_ARCH -join ' ') -ne 'x64') {
    throw 'The installed MSVC toolset, SDK or architecture does not match portablemsvc.lock.'
}

function Resolve-KitPath($path) {
    if ([IO.Path]::IsPathRooted($path)) { return $path }
    Join-Path $kit $path
}

# Consume the kit's environment directly; older activation scripts fail on PowerShell 5.1.
foreach ($name in 'CC', 'CXX', 'AR', 'VCINSTALLDIR', 'VCToolsInstallDir', 'WindowsSDKDir') {
    [Environment]::SetEnvironmentVariable($name, (Resolve-KitPath $spec.$name), 'Process')
}
foreach ($name in 'VCToolsVersion', 'WindowsSDKVersion') {
    [Environment]::SetEnvironmentVariable($name, $spec.$name, 'Process')
}
foreach ($name in 'PATH', 'INCLUDE', 'LIB', 'LIBPATH') {
    $paths = @($spec.$name | ForEach-Object { Resolve-KitPath $_ })
    if ($name -eq 'PATH') { $paths += $env:PATH }
    [Environment]::SetEnvironmentVariable($name, ($paths -join ';'), 'Process')
}

$linker = Join-Path ([IO.Path]::GetDirectoryName($env:CC)) 'link.exe'
if (-not (Test-Path -LiteralPath $linker -PathType Leaf)) {
    throw 'The locked MSVC linker is missing.'
}
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = $linker

& cargo.exe @args
exit $LASTEXITCODE
