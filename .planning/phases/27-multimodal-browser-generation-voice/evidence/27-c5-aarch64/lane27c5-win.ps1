$ErrorActionPreference = "Continue"
$W = "D:\lane-27c5"
Set-Location $W
Remove-Item -Recurse -Force "$W\a64" -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force "$W\x64" -ErrorAction SilentlyContinue
Expand-Archive -Path "$W\wayland-core-v0.12.25-aarch64-pc-windows-msvc.zip" -DestinationPath "$W\a64" -Force
Expand-Archive -Path "$W\wayland-core-v0.12.25-x86_64-pc-windows-msvc.zip" -DestinationPath "$W\x64" -Force

$lines = @()
$lines += "HOST_ARCH=$env:PROCESSOR_ARCHITECTURE"
$lines += "HOST_CPU=" + (Get-CimInstance Win32_Processor).Name
$lines += "OS=" + (Get-CimInstance Win32_OperatingSystem).Caption

foreach ($z in @("aarch64","x86_64")) {
  $zip = "$W\wayland-core-v0.12.25-$z-pc-windows-msvc.zip"
  $lines += "SHA256_$z=" + (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
}

$targets = @(
  @{ tag = "aarch64"; bin = "$W\a64\wayland-core.exe" },
  @{ tag = "x86_64";  bin = "$W\x64\wayland-core.exe" }
)

foreach ($t in $targets) {
  $tag = $t.tag
  $bin = $t.bin
  $bytes = [System.IO.File]::ReadAllBytes($bin)
  $peoff = [BitConverter]::ToInt32($bytes, 0x3C)
  $machine = [BitConverter]::ToUInt16($bytes, $peoff + 4)
  $lines += ("PE_MACHINE_" + $tag + "=0x" + $machine.ToString("X4"))
  $lines += ("SIZE_" + $tag + "=" + $bytes.Length)

  $so = "$W\$tag-stdout.txt"
  $se = "$W\$tag-stderr.txt"
  Remove-Item $so, $se -ErrorAction SilentlyContinue
  $rc = "NOLAUNCH"
  try {
    $pr = Start-Process -FilePath $bin -ArgumentList "--version" -NoNewWindow -Wait -PassThru -RedirectStandardOutput $so -RedirectStandardError $se
    $rc = $pr.ExitCode
  } catch {
    $msg = $_.Exception.Message -replace "`r", "" -replace "`n", " | "
    $lines += ("LAUNCH_EXCEPTION_" + $tag + "=" + $msg)
  }
  $lines += ("EXITCODE_" + $tag + "=" + $rc)

  $out = ""
  if (Test-Path $so) { $out = (Get-Content $so -Raw) }
  if ([string]::IsNullOrWhiteSpace($out) -and (Test-Path $se)) { $out = (Get-Content $se -Raw) }
  if ($null -eq $out) { $out = "" }
  $out = ($out -replace "`r", "" -replace "`n", " | ").Trim()
  $lines += ("OUTPUT_" + $tag + "=" + $out)
}

$lines += "WLDONE"
$lines | Set-Content -Path "$W\lane27c5-win-status.txt" -Encoding utf8
Write-Host "WROTE"
