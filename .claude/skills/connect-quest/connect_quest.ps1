<#
.SYNOPSIS
  Bring the Quest headset online over adb on the remote OpenXR test host (.101),
  so the diagnostic APK can be installed and client logcat captured.

.DESCRIPTION
  Runs entirely against the remote host via PS remoting. On the remote it:
    1. locates adb,
    2. determines the Quest IP (explicit param -> cached quest_ip.txt ->
       most-recent client IP in session_log.txt -> subnet scan for TCP 5555),
    3. `adb connect <ip>:5555`,
    4. verifies with `adb devices`, and caches the working IP.

  Wireless adb requires the Quest to already be in Wireless-Debug / tcpip mode
  and previously authorized. If it was rebooted out of tcpip mode this script
  cannot recover it without a one-time USB `adb tcpip 5555` (physical action).

.EXAMPLE
  ./connect_quest.ps1
  ./connect_quest.ps1 -QuestIp 192.168.10.50
  ./connect_quest.ps1 -RemoteHost 192.168.10.101 -Subnet 192.168.10
#>
param(
  [string]$RemoteHost = '192.168.10.101',
  [string]$QuestIp = '',
  [string]$Subnet = '192.168.10',
  [string]$AdbPath = 'C:\Users\worker\AppData\Local\Android\Sdk\platform-tools\adb.exe',
  [string]$TestDir = 'C:\alvr\test-openxr',
  [string]$AlvrRootRel = 'alvr_openxr_root'  # under %LOCALAPPDATA% on the remote
)

Invoke-Command -ComputerName $RemoteHost -ArgumentList $QuestIp,$Subnet,$AdbPath,$TestDir,$AlvrRootRel -ScriptBlock {
  param($QuestIp,$Subnet,$adb,$TestDir,$AlvrRootRel)

  if (-not (Test-Path $adb)) { return "ERROR: adb not found at $adb" }
  $cacheFile = Join-Path $TestDir 'quest_ip.txt'
  $log = @()

  function Try-Adb { param($a) ((& $adb @a 2>&1) | Out-String).Trim() }

  # --- resolve candidate IP ---
  $candidates = @()
  if ($QuestIp) { $candidates += $QuestIp }
  if (-not $candidates -and (Test-Path $cacheFile)) {
    $c = (Get-Content $cacheFile -Raw).Trim()
    if ($c) { $candidates += $c; $log += "cache: $c" }
  }
  if (-not $candidates) {
    $sl = Join-Path $env:LOCALAPPDATA "$AlvrRootRel\session_log.txt"
    if (Test-Path $sl) {
      $ips = Select-String -Path $sl -Pattern '(\d{1,3}\.){3}\d{1,3}' -AllMatches -Encoding utf8 |
        ForEach-Object { $_.Matches.Value } |
        Where-Object { $_ -like "$Subnet.*" -and $_ -notlike '*.255' } |
        Select-Object -Unique
      if ($ips) { $candidates += ($ips | Select-Object -Last 1); $log += "session_log ip(s): $($ips -join ',')" }
    }
  }
  if (-not $candidates) {
    # fast async scan of the subnet for TCP 5555
    $log += "scanning $Subnet.0/24 for tcp/5555 ..."
    $found = @()
    1..254 | ForEach-Object {
      $ip = "$Subnet.$_"
      $t = New-Object System.Net.Sockets.TcpClient
      try { if ($t.ConnectAsync($ip,5555).Wait(120)) { $found += $ip } } catch {} finally { $t.Dispose() }
    }
    if ($found) { $candidates += $found; $log += "open 5555: $($found -join ',')" }
  }

  if (-not $candidates) { return ($log + "RESULT: no Quest IP candidates found (connect USB once and run `adb tcpip 5555`).") -join "`n" }

  # --- connect ---
  $connected = $null
  foreach ($ip in $candidates) {
    $r = Try-Adb @('connect',"$($ip):5555")
    $log += "adb connect $($ip):5555 -> $r"
    Start-Sleep -Milliseconds 600
    $dev = Try-Adb @('devices')
    if ($dev -match "$([regex]::Escape($ip)):5555\s+device") { $connected = $ip; break }
  }

  $log += "=== adb devices ==="
  $log += (Try-Adb @('devices'))
  if ($connected) {
    Set-Content -Path $cacheFile -Value $connected -Encoding ascii
    $log += "RESULT: CONNECTED $connected (cached to quest_ip.txt)"
  } else {
    $log += "RESULT: NOT CONNECTED. If the only state is 'offline'/'unauthorized', accept the in-headset USB-debug prompt; if no device, the Quest is not in wireless-debug/tcpip mode."
  }
  $log -join "`n"
}
