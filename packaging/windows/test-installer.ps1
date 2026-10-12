# Installs, updates and removes TonePush's Windows packages on a CI runner,
# with stand-in program files: the Inno Setup installer of 0.9.x, the MSI
# taking it over (as the updater runs it, and by hand), an MSI update, and
# uninstalling with TonePush running and then without.
#
#   pwsh packaging/windows/test-installer.ps1
#
# Needs Inno Setup 6, WiX 5 with WixToolset.Util.wixext, and Ruby. Run from
# the repository root. Changes the current user's TonePush install.

$ErrorActionPreference = "Stop"
$root = Join-Path $env:RUNNER_TEMP "tonepush-installer-test"
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
New-Item -ItemType Directory $root | Out-Null

$innoKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{E7F9B874-71BE-4DE7-8323-BB09AFBD702F}_is1"
$startMenu = Join-Path ([Environment]::GetFolderPath("Programs")) "TonePush.lnk"
$desktop = Join-Path ([Environment]::GetFolderPath("Desktop")) "TonePush.lnk"
$defaultFolder = Join-Path $env:LOCALAPPDATA "Programs\TonePush"

function Check($condition, $what) {
  if (-not $condition) { throw "FAILED: $what" }
  Write-Host "ok: $what"
}

# Program files without a version resource, dated before their install as a
# release build is, which is what keeps Windows Installer from replacing them
# unless the package says to.
function Payload($version) {
  $dir = Join-Path $root "payload-$version"
  New-Item -ItemType Directory -Force $dir | Out-Null
  foreach ($exe in "tonepush-gui.exe", "tonepush.exe") {
    Set-Content -NoNewline -Path (Join-Path $dir $exe) -Value "$exe $version"
  }
  Copy-Item README.md, LICENSE, packaging\windows\tonepush-installer.txt $dir
  Get-ChildItem $dir | ForEach-Object { $_.LastWriteTime = (Get-Date).AddDays(-30) }
  $dir
}

function Inno($version, $arguments) {
  $iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
  if (-not $iscc) { $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
  & $iscc /Q "/DVersion=$version" /DArch=x86_64 "/DBinaries=$(Payload "inno-$version")" `
    "/DOutputDir=$root" packaging\windows\tonepush.iss
  if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
  $setup = Join-Path $root "tonepush-v$version-x86_64-pc-windows-msvc-setup.exe"
  $process = Start-Process $setup -ArgumentList $arguments -Wait -PassThru
  Check ($process.ExitCode -eq 0) "Inno Setup $version installs"
}

function Msi($version, $arch = "x64") {
  $package = Join-Path $root "tonepush-$version-$arch.msi"
  ruby packaging\windows\msi.rb (Payload "msi-$version-$arch") $package $version $arch | Out-Host
  if ($LASTEXITCODE -ne 0) { throw "MSI build failed" }
  $package
}

function Msiexec($arguments) {
  $log = Join-Path $root "msiexec-$([guid]::NewGuid()).log"
  $script:lastLog = $log
  $process = Start-Process msiexec.exe -ArgumentList "$arguments /l*v `"$log`"" -Wait -PassThru
  if ($process.ExitCode -notin 0, 3010) {
    Get-Content $log -Tail 60 -ErrorAction SilentlyContinue | Write-Host
  }
  $process.ExitCode
}

function Program($folder) {
  Get-Content -Raw (Join-Path $folder "tonepush-gui.exe")
}

# What Settings > Apps lists for TonePush: the MSI installs Windows Installer
# knows by TonePush's UpgradeCode, and the Inno Setup entry.
function Entries {
  $installer = New-Object -ComObject WindowsInstaller.Installer
  $products = $installer.GetType().InvokeMember("RelatedProducts", "GetProperty", $null, $installer,
    @("{B4344637-0747-4C62-BB78-1BD669BAE353}"))
  $count = @($products).Count
  if (Test-Path $innoKey) { $count += 1 }
  $count
}

# 1. The Inno Setup installer, with its desktop shortcut.
Inno "0.9.0" "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER /TASKS=desktopicon"
Check (Test-Path (Join-Path $defaultFolder "unins000.exe")) "Inno installs its uninstaller"
Check (Test-Path $innoKey) "Inno registers in Settings > Apps"
Check (Test-Path $desktop) "Inno makes the desktop shortcut"

# 2. The MSI, run the way the updater runs it.
$v1 = Msi "0.9.1"
$code = Msiexec "/i `"$v1`" /qn /norestart INSTALLFOLDER=`"$defaultFolder`""
Check ($code -in 0, 3010) "the MSI takes over the Inno install ($code)"
Check ((Program $defaultFolder) -eq "tonepush-gui.exe msi-0.9.1-x64") "the editor is replaced"
Check (Test-Path (Join-Path $defaultFolder "tonepush-installer.txt")) "the installer marker is in place"
Check (-not (Test-Path (Join-Path $defaultFolder "unins000.exe"))) "Inno's uninstaller is gone"
Check (-not (Test-Path (Join-Path $defaultFolder "unins000.dat"))) "Inno's uninstall log is gone"
Check (-not (Test-Path $innoKey)) "Inno's Settings > Apps entry is gone"
Check ((Entries) -eq 1) "Settings > Apps lists TonePush once"
Check (Test-Path $startMenu) "the Start menu shortcut is there"
Check (Test-Path $desktop) "the desktop shortcut is kept"

# 3. An MSI update run by hand finds the install on its own.
$v2 = Msi "0.9.2"
$code = Msiexec "/i `"$v2`" /qn /norestart"
Check ($code -in 0, 3010) "the next MSI updates in place ($code)"
Check ((Program $defaultFolder) -eq "tonepush-gui.exe msi-0.9.2-x64") "the editor is the new one"
Check ((Entries) -eq 1) "Settings > Apps still lists TonePush once"
Check (Test-Path $desktop) "the desktop shortcut survives the update"

# 4. Uninstalling while the tonepush command runs is refused, untouched.
$fake = Join-Path $root "running"
New-Item -ItemType Directory $fake | Out-Null
Copy-Item "$env:SystemRoot\System32\PING.EXE" (Join-Path $fake "tonepush.exe")
$running = Start-Process (Join-Path $fake "tonepush.exe") -ArgumentList "-n 600 127.0.0.1" -PassThru -WindowStyle Hidden
try {
  $code = Msiexec "/x `"$v2`" /qn /norestart"
  Check ($code -ne 0 -and $code -ne 3010) "uninstalling while TonePush runs is refused ($code)"
  Check (Select-String -Quiet -Encoding unicode "TonePush is running" $script:lastLog) "by the check for a running TonePush"
  Check ((Program $defaultFolder) -eq "tonepush-gui.exe msi-0.9.2-x64") "nothing was removed"
} finally {
  Stop-Process -Id $running.Id -Force
}

# 5. Uninstalling once it has quit removes everything.
$code = Msiexec "/x `"$v2`" /qn /norestart"
Check ($code -in 0, 3010) "uninstalling removes TonePush ($code)"
Check (-not (Test-Path $defaultFolder)) "the program folder is gone"
Check (-not (Test-Path $startMenu)) "the Start menu shortcut is gone"
Check (-not (Test-Path $desktop)) "the desktop shortcut is gone"
Check ((Entries) -eq 0) "Settings > Apps no longer lists TonePush"

# 6. An Inno install in a folder of its own, taken over by an MSI run by hand.
$custom = Join-Path $env:LOCALAPPDATA "TonePush Elsewhere"
Inno "0.9.0" "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER `"/DIR=$custom`""
$code = Msiexec "/i `"$v1`" /qn /norestart"
Check ($code -in 0, 3010) "a hand-run MSI takes over the Inno install ($code)"
Check ((Program $custom) -eq "tonepush-gui.exe msi-0.9.1-x64") "it installs into Inno's folder"
Check (-not (Test-Path $defaultFolder)) "and not into the default one"
Check (-not (Test-Path $desktop)) "no desktop shortcut appears that Inno did not make"
$code = Msiexec "/x `"$v1`" /qn /norestart"
Check ($code -in 0, 3010) "and uninstalls ($code)"
Check (-not (Test-Path $custom)) "leaving no folder behind"

# 7. The Windows on ARM package builds.
Msi "0.9.1" "arm64" | Out-Null
Check $true "the arm64 MSI builds"
