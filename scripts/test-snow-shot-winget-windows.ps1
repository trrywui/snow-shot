#Requires -Version 7.0
# Offscreen native windows only; no installer or interaction with other processes.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'snow-shot-winget-test-windows.ps1')
if (-not ('SnowShotWingetWindowFixture' -as [type])) {
    Add-Type @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class SnowShotWingetWindowFixture {
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateWindowEx(uint extendedStyle, string className, string name,
        uint style, int x, int y, int width, int height, IntPtr parent, IntPtr menu, IntPtr instance, IntPtr data);
    [DllImport("user32.dll")] public static extern bool DestroyWindow(IntPtr window);
    public static IntPtr Create(string title, IntPtr owner, bool visible) {
        var window = CreateWindowEx(0, "STATIC", title, 0x80000000U | (visible ? 0x10000000U : 0),
            -32000, -32000, 100, 100, owner, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
        if (window == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        return window;
    }
}
'@
}
$windows = [Collections.Generic.List[IntPtr]]::new()
try {
    $container = [SnowShotWingetWindowFixture]::Create('Window Dialog', [IntPtr]::Zero, $true)
    $windows.Add($container)
    $warning = [SnowShotWingetWindowFixture]::Create('Open File - Security Warning', $container, $true)
    $windows.Add($warning)
    $hidden = [SnowShotWingetWindowFixture]::Create('Hidden launch helper', [IntPtr]::Zero, $false)
    $windows.Add($hidden)
    $process = [Diagnostics.Process]::GetCurrentProcess()
    $process.Refresh()
    if ($process.MainWindowHandle -eq $warning) {
        throw 'The fixture must reproduce a consent dialog excluded by the main window lookup.'
    }
    $visible = @([WinGetWindowDiagnostics]::ForProcess($PID))
    if ($container -notin $visible -or $warning -notin $visible -or $hidden -in $visible) {
        throw 'WinGet window enumeration must include visible owned launch warnings and exclude hidden windows.'
    }
    if ([WinGetWindowDiagnostics]::Title($warning) -cne 'Open File - Security Warning' -or
        [WinGetWindowDiagnostics]::Read($warning) -cne 'Open File - Security Warning') {
        throw 'Owned launch warning text must remain available for failure diagnostics.'
    }
    if (@([WinGetWindowDiagnostics]::ForProcess(-1)).Count) {
        throw 'Unrelated processes must not supply diagnostic windows.'
    }
    Write-Output 'PASS: WinGet owned launch warning diagnostics with offscreen native windows.'
} finally {
    for ($index = $windows.Count - 1; $index -ge 0; $index--) {
        $null = [SnowShotWingetWindowFixture]::DestroyWindow($windows[$index])
    }
}
