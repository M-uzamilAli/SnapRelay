# Builds the release: dist\SnapRelay-Setup-<version>.exe and dist\SnapRelay-<version>-portable.zip
# Needs Rust and Inno Setup 6 (winget install JRSoftware.InnoSetup).
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
Write-Host "SnapRelay $version"

cargo build --release
if ($LASTEXITCODE) { throw 'cargo build failed' }

$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found (winget install JRSoftware.InnoSetup)' }
& $iscc /Qp "/DAppVersion=$version" installer\snaprelay.iss
if ($LASTEXITCODE) { throw 'installer build failed' }

# Portable: the program plus the font licence, nothing to install.
$stage = Join-Path $env:TEMP "snaprelay-portable-$version"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $stage | Out-Null
Copy-Item target\release\snaprelay.exe $stage
Copy-Item assets\fonts\Inter-LICENSE.txt (Join-Path $stage 'Inter-font-LICENSE.txt')
$zip = "dist\SnapRelay-$version-portable.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage\*" -DestinationPath $zip
Remove-Item $stage -Recurse -Force

Get-ChildItem dist | ForEach-Object {
    '{0,-36} {1,8:N1} MB  sha256 {2}' -f $_.Name, ($_.Length / 1MB), (Get-FileHash $_.FullName -Algorithm SHA256).Hash
}
