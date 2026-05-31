---
name: connect-quest
description: Bring the Quest headset online over adb on the remote OpenXR test host (.101) so the diagnostic client APK can be installed and client logcat captured. Use before any OpenXR-mode headset smoke (AK/hello_xr on Monado) when `adb devices` on .101 is empty. Discovers the Quest IP (cached -> session_log -> subnet scan), `adb connect`s it, and caches the working IP.
---

# connect-quest

Connects the standalone Quest to the remote OpenXR test host **192.168.10.101**
over wireless adb, which is the precondition for installing the ALVR client APK
and reading client-side `adb logcat` during an OpenXR-mode smoke.

## When to use
- Before running an OpenXR-mode headset smoke and `adb devices` on .101 shows
  "List of devices attached" with nothing under it.
- After a .101 reboot / adb daemon restart dropped the wireless device.

## How to run
From the build host (`D:\projects\alvr`):

```powershell
pwsh -File .claude/skills/connect-quest/connect_quest.ps1
# or, if the IP is known / changed:
pwsh -File .claude/skills/connect-quest/connect_quest.ps1 -QuestIp 192.168.10.50
```

(Use `powershell.exe -File ...` on Windows PowerShell 5.1.)

The script runs entirely against .101 via PS remoting. It:
1. locates adb at `C:\Users\worker\AppData\Local\Android\Sdk\platform-tools\adb.exe`,
2. resolves the Quest IP: `-QuestIp` arg → `C:\alvr\test-openxr\quest_ip.txt`
   cache → most-recent `192.168.10.*` in
   `%LOCALAPPDATA%\alvr_openxr_root\session_log.txt` → async subnet scan for
   open TCP 5555,
3. `adb connect <ip>:5555`,
4. verifies via `adb devices` and caches the working IP to `quest_ip.txt`.

## Output to read
- `RESULT: CONNECTED <ip>` → proceed to the smoke (install APK, launch
  monado-service + AK, launch client, capture logcat + session_log).
- `RESULT: NOT CONNECTED` → the device list is empty or `unauthorized`/`offline`.

## Limitations (physical action may be required)
Wireless adb needs the Quest already in **Wireless Debug / tcpip** mode and
previously authorized. If it was rebooted out of tcpip mode, recovery needs a
one-time **USB** connection + `adb tcpip 5555` + accepting the in-headset
"Allow USB debugging" prompt — that cannot be done remotely.

## Notes / gotchas
- adb writes its "daemon not running; starting now" banner to **stderr**; under
  Windows PowerShell that surfaces as a `NativeCommandError` and can cancel a
  batched tool call. The script captures adb with `2>&1 | Out-String` so this
  stays contained — when calling adb ad-hoc, do the same and don't batch a raw
  `adb` call with other tools.
- Non-elevated only: the Monado IPC pipe ACL and the bundled-loader HKLM/HKCU
  quirk both require the whole OpenXR-mode stack to run un-elevated.
