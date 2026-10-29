#Requires -Version 7.0
if ('WinGetWindowDiagnostics' -as [type]) { return }
Add-Type @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class WinGetWindowDiagnostics {
    private delegate bool Callback(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] private static extern bool EnumWindows(Callback callback, IntPtr data);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] private static extern bool EnumChildWindows(IntPtr window, Callback callback, IntPtr data);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder text, int count);
    public static IntPtr[] ForProcess(int processId) {
        var windows = new List<IntPtr>();
        EnumWindows((window, data) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == processId && IsWindowVisible(window)) windows.Add(window);
            return true;
        }, IntPtr.Zero);
        return windows.ToArray();
    }
    public static string Title(IntPtr window) {
        var text = new StringBuilder(2048);
        GetWindowText(window, text, text.Capacity);
        return text.ToString();
    }
    public static string Read(IntPtr window) {
        var lines = new List<string>();
        Callback callback = (child, data) => {
            var text = new StringBuilder(2048);
            GetWindowText(child, text, text.Capacity);
            if (text.Length > 0) lines.Add(text.ToString());
            return true;
        };
        callback(window, IntPtr.Zero);
        EnumChildWindows(window, callback, IntPtr.Zero);
        return string.Join(Environment.NewLine, lines);
    }

}
'@
