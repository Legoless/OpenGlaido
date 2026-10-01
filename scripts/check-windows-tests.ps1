# Tauri's Common Controls manifest must also be embedded in the lib test harness.
# Without it ComCtl32 v5 cannot resolve the v6 imports and tests exit with 0xc0000139.
$ErrorActionPreference = 'Stop'
$diagnostics = Join-Path $env:RUNNER_TEMP 'openglaido-loader'
New-Item -ItemType Directory -Force $diagnostics | Out-Null
$artifacts = Join-Path $diagnostics 'test-artifacts.jsonl'
cargo test --manifest-path src-tauri/Cargo.toml --locked --no-run --message-format=json > $artifacts
if ($LASTEXITCODE -ne 0) { throw 'Building Rust test executables failed' }

$tests = @(Get-Content $artifacts | ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable })
if ($tests.Count -eq 0) { throw 'Cargo produced no test executables' }
$mt = Get-ChildItem "${env:ProgramFiles(x86)}/Windows Kits/10/bin/*/x64/mt.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
if (!$mt) { throw 'Windows SDK manifest tool was not found' }

foreach ($test in $tests) {
    $manifest = Join-Path $diagnostics ((Split-Path $test.executable -Leaf) + '.manifest')
    & $mt -nologo "-inputresource:$($test.executable);#1" "-out:$manifest"
    if ($LASTEXITCODE -ne 0) { throw "No embedded application manifest in $($test.executable)" }
    [xml]$xml = Get-Content -Raw $manifest
    $dependency = $xml.assembly.dependency.dependentAssembly.assemblyIdentity |
        Where-Object { $_.name -eq 'Microsoft.Windows.Common-Controls' -and $_.version -eq '6.0.0.0' }
    if (!$dependency) { throw "Common Controls v6 is missing from $($test.executable)" }
    Write-Host "Verified Common Controls v6 in $($test.executable)"
}
