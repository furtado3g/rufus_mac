param(
    [Parameter(Mandatory = $true)][string]$Version
)

$ErrorActionPreference = "Stop"
$Dist = "dist"
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

# Release CI runs this on native x64 and arm64 Windows runners, so the host
# target is the architecture we ship. This avoids depending on ARM64 MSVC
# libraries being present in an x64 cross toolchain.
$Target = ((rustc -vV | Select-String '^host: ').ToString() -replace '^host: ', '').Trim()
switch -Regex ($Target) {
    '^x86_64-pc-windows-msvc$' { $Arch = "x64" }
    '^aarch64-pc-windows-msvc$' { $Arch = "arm64" }
    default { throw "unsupported host target: $Target" }
}

cargo build --release --locked -p image2usb-cli -p image2usb-gui --target $Target
if ($LASTEXITCODE -ne 0) { throw "cargo build failed for $Target" }

$Stage = Join-Path $env:TEMP "image2usb-$Arch"
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

Copy-Item "target/$Target/release/image2usb.exe" $Stage
Copy-Item "target/$Target/release/image2usb-gui.exe" $Stage
Copy-Item LICENSE $Stage
Copy-Item THIRD_PARTY_NOTICES.md $Stage
Copy-Item README.md $Stage

$Archive = Join-Path $Dist "image2usb-$Version-windows-$Arch.zip"
Remove-Item -Force -ErrorAction SilentlyContinue $Archive
Compress-Archive -Path (Join-Path $Stage "*") -DestinationPath $Archive
Remove-Item -Recurse -Force $Stage
