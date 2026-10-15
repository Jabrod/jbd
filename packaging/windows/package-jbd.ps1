<#
.SYNOPSIS
  Build and package JBD for Windows: the jabrod.com installer and the Microsoft Store package.

.DESCRIPTION
  Produces, in dist\jbd-<version>\:
    JBD-<version>-setup.exe      per-user installer for the jabrod.com download (NSIS)
    JBD_<version>.0_x64.msix     unsigned Store package (Partner Center signs it)
    jbd.exe                      the program itself (portable)
    release.json                 version, sizes and SHA-256s (for jabrod.com, lib/jbd-release.ts)

  jbd.exe links the C runtime statically (.cargo/config.toml), so neither package needs the
  Visual C++ redistributable.

  Needs: Rust (MSVC), Node.js (the notices script), the Windows SDK (MakeAppx), NSIS
  (makensis; the copy Tauri downloaded to %LOCALAPPDATA%\tauri\NSIS works).

.EXAMPLE
  pwsh packaging/windows/package-jbd.ps1
  pwsh packaging/windows/package-jbd.ps1 -SkipBuild
  pwsh packaging/windows/package-jbd.ps1 -SkipBuild -StoreOnly -StoreVersion 1.0.1.0

  -StoreVersion sets the MSIX version on its own (a.b.c.0; Partner Center needs the fourth field 0
  and a new version for every package uploaded to the product, even one never published).
  -StoreOnly remakes only the MSIX and keeps the rest of dist (the signed installer) as it is.
#>
param(
  [switch] $SkipBuild,
  [string] $MakeNsis,
  [string] $StoreVersion,
  [switch] $StoreOnly
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  # Native tools (cargo, makensis) write progress to stderr; Windows PowerShell 5.1 turns that into
  # error records, which 'Stop' would treat as failure. Judge them by their exit code instead.
  $saved = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try { & $Block 2>&1 | ForEach-Object { "$_" } } finally { $ErrorActionPreference = $saved }
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# the version: [workspace.package] version in Cargo.toml
$Version = $null
$inPkg = $false
foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
  if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
  if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
$Plain = ($Version -split '-')[0]
Write-Output "JBD $Version"

$Dist = Join-Path $Root "dist\jbd-$Version"
if (-not $StoreOnly) { Remove-Item -Recurse -Force $Dist -ErrorAction SilentlyContinue }
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

if (-not $SkipBuild) {
  # fail (rather than warn) if the icon / version info can't be embedded in jbd.exe
  $env:FILMCRAFT_REQUIRE_WINRES = '1'
  Invoke-Native 'cargo build (release)' { cargo build --release --locked -p filmcraft -p filmcraft-cli }
}
$Exe = Join-Path $Root 'target\release\jbd.exe'
# JBD's MCP program: lets Claude Code / Claude Desktop drive JBD (Settings > Connections)
$Mcp = Join-Path $Root 'target\release\filmcraft-cli.exe'
if (-not (Test-Path $Mcp)) { throw "filmcraft-cli.exe not found: $Mcp" }
if (-not (Test-Path $Exe)) { throw "jbd.exe not found: $Exe" }

# jbd.exe must be a 64-bit Windows GUI program (no console window)
$bytes = [System.IO.File]::ReadAllBytes($Exe)
$pe = [BitConverter]::ToInt32($bytes, 0x3C)
$machine = [BitConverter]::ToUInt16($bytes, $pe + 4)
$subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C)
if ($machine -ne 0x8664) { throw ('jbd.exe is for machine 0x{0:X}, expected x64' -f $machine) }
if ($subsystem -ne 2) { throw "jbd.exe has PE subsystem $subsystem, expected 2 (Windows GUI)" }
$info = (Get-Item $Exe).VersionInfo
if ($info.ProductName -ne 'JBD') { throw "jbd.exe has no JBD version info (ProductName '$($info.ProductName)')" }
Write-Output "ok jbd.exe: x64, GUI, ProductName JBD"

# the files every package ships
$Stage = Join-Path $Root 'target\jbd-package\stage'
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item $Exe $Stage
Copy-Item $Mcp (Join-Path $Stage 'jbd-mcp.exe')
Copy-Item (Join-Path $PSScriptRoot 'EULA.txt') $Stage
Invoke-Native 'third-party notices' { node (Join-Path $PSScriptRoot 'notices.cjs') (Join-Path $Stage 'THIRD-PARTY-NOTICES.txt') }
Copy-Item $Exe $Dist

# ---- the jabrod.com installer (NSIS) -------------------------------------------------------------
if (-not $StoreOnly) {
if (-not $MakeNsis) {
  $found = Get-Command makensis -ErrorAction SilentlyContinue
  if ($found) { $MakeNsis = $found.Source } else { $MakeNsis = Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe' }
}
if (-not (Test-Path $MakeNsis)) { throw "makensis not found ($MakeNsis). Install NSIS 3 or pass -MakeNsis." }
$Setup = Join-Path $Dist "JBD-$Version-setup.exe"
$SizeKb = [int][math]::Ceiling(((Get-ChildItem $Stage | Measure-Object Length -Sum).Sum) / 1024)
Invoke-Native 'makensis' {
  & $MakeNsis /V2 "/DVER=$Plain" "/DSTAGE=$Stage" "/DICON=$(Join-Path $Root 'assets\jbd\jbd.ico')" "/DOUT=$Setup" "/DSIZE_KB=$SizeKb" (Join-Path $PSScriptRoot 'jbd.nsi')
}
}

# ---- the Microsoft Store package (MSIX) -----------------------------------------------------------
$MsixVersion = "$Plain.0"
if ($StoreVersion) {
  if ($StoreVersion -notmatch '^\d+\.\d+\.\d+\.0$') { throw "-StoreVersion must look like 1.0.1.0 (fourth field 0), got '$StoreVersion'" }
  $MsixVersion = $StoreVersion
}
# one Store package per dist folder: drop earlier MSIX builds
Get-ChildItem $Dist -Filter 'JBD_*_x64.msix' -ErrorAction SilentlyContinue | Remove-Item -Force
$Msix = Join-Path $Dist "JBD_${MsixVersion}_x64.msix"
$MsixStage = Join-Path $Root 'target\jbd-package\msix'
Remove-Item -Recurse -Force $MsixStage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path (Join-Path $MsixStage 'Assets') | Out-Null
Copy-Item (Join-Path $Stage '*') $MsixStage
Copy-Item (Join-Path $Root 'assets\jbd\store\*.png') (Join-Path $MsixStage 'Assets')
# the 310x150 wide tile: the full-bleed icon centred with transparent sides (never stretched)
Add-Type -AssemblyName System.Drawing
$src = [System.Drawing.Image]::FromFile((Join-Path $Root 'assets\jbd\jbd-icon-1024.png'))
$wide = New-Object System.Drawing.Bitmap 310, 150
$g = [System.Drawing.Graphics]::FromImage($wide)
try {
  $g.Clear([System.Drawing.Color]::Transparent)
  $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $g.DrawImage($src, 80, 0, 150, 150)
  $wide.Save((Join-Path $MsixStage 'Assets\Wide310x150Logo.png'), [System.Drawing.Imaging.ImageFormat]::Png)
} finally { $g.Dispose(); $wide.Dispose(); $src.Dispose() }
$manifest = (Get-Content -Raw (Join-Path $PSScriptRoot 'store\AppxManifest.template.xml')).Replace('__PACKAGE_VERSION__', $MsixVersion)
[System.IO.File]::WriteAllText((Join-Path $MsixStage 'AppxManifest.xml'), $manifest, (New-Object System.Text.UTF8Encoding($false)))
$kits = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
$MakeAppx = Get-ChildItem $kits -Directory | Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } | Sort-Object { [version]$_.Name } -Descending |
  ForEach-Object { Join-Path $_.FullName 'x64\makeappx.exe' } | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $MakeAppx) { throw 'makeappx.exe not found: install the Windows 10/11 SDK' }
# resources.pri indexes the Assets' size variants (Square44x44Logo.targetsize-N_altform-unplated),
# so the taskbar and Start show the full-size icon instead of the small plated 44 px one
$MakePri = Join-Path (Split-Path $MakeAppx) 'makepri.exe'
if (-not (Test-Path $MakePri)) { throw 'makepri.exe not found: install the Windows 10/11 SDK' }
$PriConfig = Join-Path $Root 'target\jbd-package\priconfig.xml'
Invoke-Native 'makepri createconfig' { & $MakePri createconfig /cf $PriConfig /dq en-US /pv 10.0.0 /o }
Invoke-Native 'makepri new' { & $MakePri new /pr $MsixStage /cf $PriConfig /mn (Join-Path $MsixStage 'AppxManifest.xml') /of (Join-Path $MsixStage 'resources.pri') /o }
Invoke-Native 'makeappx pack' { & $MakeAppx pack /d $MsixStage /p $Msix /o }
# read it back and check the identity Partner Center expects
$Verify = Join-Path $Root 'target\jbd-package\msix-verify'
Remove-Item -Recurse -Force $Verify -ErrorAction SilentlyContinue
Invoke-Native 'makeappx unpack (verify)' { & $MakeAppx unpack /p $Msix /d $Verify /o | Out-Null }
[xml]$packed = Get-Content (Join-Path $Verify 'AppxManifest.xml')
$id = $packed.Package.Identity
if ($id.Name -ne 'Jabrod.JBD' -or $id.Publisher -ne 'CN=B31502B4-249B-4B91-8E2D-5E7989531A54' -or $id.Version -ne $MsixVersion) {
  throw 'the MSIX identity does not match Partner Center'
}
Remove-Item -Recurse -Force $Verify

# ---- release.json ---------------------------------------------------------------------------------
$files = foreach ($f in Get-ChildItem $Dist -File) {
  [ordered]@{ name = $f.Name; bytes = $f.Length; sha256 = (Get-FileHash $f.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
}
[ordered]@{ product = 'jbd'; version = $Version; builtAt = (Get-Date).ToUniversalTime().ToString('o'); files = @($files) } |
  ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 (Join-Path $Dist 'release.json')
Get-ChildItem $Dist | Format-Table Name, Length
