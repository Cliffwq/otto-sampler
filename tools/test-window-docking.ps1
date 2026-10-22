param([Parameter(Mandatory=$true)][int]$AppProcessId)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class DockProbe {
 [StructLayout(LayoutKind.Sequential)] public struct RECT {public int Left,Top,Right,Bottom;}
 [StructLayout(LayoutKind.Sequential)] public struct MONITORINFO {public int cbSize;public RECT rcMonitor,rcWork;public uint flags;}
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd,out RECT rect);
 [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr hwnd,uint flags);
 [DllImport("user32.dll")] public static extern bool GetMonitorInfo(IntPtr monitor,ref MONITORINFO info);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd,IntPtr after,int x,int y,int cx,int cy,uint flags);
 [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeout(IntPtr hwnd,uint msg,IntPtr w,IntPtr l,uint flags,uint timeout,out IntPtr result);
}
'@
[DockProbe]::SetThreadDpiAwarenessContext([IntPtr](-4)) | Out-Null
$appProcess = Get-Process -Id $AppProcessId
$handle = $appProcess.MainWindowHandle
if ($handle -eq 0) { throw 'No visible application window' }
$rect = New-Object DockProbe+RECT
$monitor = New-Object DockProbe+MONITORINFO
$monitor.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($monitor)
[DockProbe]::GetWindowRect($handle,[ref]$rect) | Out-Null
[DockProbe]::GetMonitorInfo([DockProbe]::MonitorFromWindow($handle,2),[ref]$monitor) | Out-Null
$width = $rect.Right-$rect.Left
$centerX = $monitor.rcWork.Left + [int](($monitor.rcWork.Right-$monitor.rcWork.Left-$width)/2)
$centerY = $monitor.rcWork.Top + 60
$positions = @(($monitor.rcMonitor.Left-60), $centerX, ($monitor.rcMonitor.Right-$width+60), $centerX)
try {
  foreach ($cycle in 1..5) {
    foreach ($x in $positions) {
      # Async positioning avoids blocking the test runner if the UI deadlocks.
      if (-not [DockProbe]::SetWindowPos($handle,[IntPtr]::Zero,$x,$centerY,0,0,0x4015)) { throw 'Position request failed' }
      Start-Sleep -Milliseconds 650
      $result = [IntPtr]::Zero
      if ([DockProbe]::SendMessageTimeout($handle,0,[IntPtr]::Zero,[IntPtr]::Zero,2,1500,[ref]$result) -eq [IntPtr]::Zero) {
        throw "Window stopped responding: cycle=$cycle x=$x"
      }
    }
  }
  'PASS: 20 native edge/undock moves, UI responds after every move'
} finally {
  [DockProbe]::SetWindowPos($handle,[IntPtr]::Zero,$centerX,$rect.Top,0,0,0x4015) | Out-Null
}
