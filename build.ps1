[CmdletBinding(DefaultParameterSetName = 'Development')]
param(
    [Parameter(ParameterSetName = 'Release')][switch]$Release,
    [Parameter(ParameterSetName = 'Optimized')][switch]$Optimized
)
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    if (-not $env:CMAKE -and -not (Get-Command cmake -ErrorAction SilentlyContinue)) {
        $locator = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        if (Test-Path -LiteralPath $locator) {
            $cmake = & $locator -latest -products '*' -find 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe' | Select-Object -First 1
            if ($cmake) { $env:CMAKE = $cmake }
        }
    }
    $arguments = @('build', '--locked')
    if ($Release) { $arguments += '--release' }
    if ($Optimized) { $arguments += @('--config', 'profile.dev.package.disqro.opt-level=1') }
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw "Disqro build failed ($LASTEXITCODE)." }
} finally {
    Pop-Location
}
