[CmdletBinding()]
param([switch]$Release)
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
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw "Disqro build failed ($LASTEXITCODE)." }
} finally {
    Pop-Location
}
