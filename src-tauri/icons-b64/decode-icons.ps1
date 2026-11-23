# Decodes the base64 icon payloads in this folder into src-tauri/icons/.
# The binary icons can't travel in the GitHub snapshot push (UTF-8 only),
# so they ride along as .b64 text files and are decoded here on the server.
# Run from C:\sonic-launcher:  powershell -ExecutionPolicy Bypass -File src-tauri\icons-b64\decode-icons.ps1
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$iconsDir = Join-Path $here "..\icons"
New-Item -ItemType Directory -Force -Path $iconsDir | Out-Null
$map = @{
  "icon.png.b64"        = "icon.png"
  "128x128.png.b64"     = "128x128.png"
  "128x128@2x.png.b64"  = "128x128@2x.png"
  "32x32.png.b64"       = "32x32.png"
  "icon.ico.b64"        = "icon.ico"
}
foreach ($pair in $map.GetEnumerator()) {
  $b64Path = Join-Path $here $pair.Key
  $outPath = Join-Path $iconsDir $pair.Value
  $b64 = Get-Content -Raw $b64Path
  [IO.File]::WriteAllBytes($outPath, [Convert]::FromBase64String($b64))
  Write-Host "decoded $($pair.Value) ($((Get-Item $outPath).Length) bytes)"
}
Write-Host "icons ready in $iconsDir"
