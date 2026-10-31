# Why does the thread-description observer fail? Check the export, the OS build and a
# declaration that binds the exact entry point.
if (-not ('ThreadNameProbe.Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace ThreadNameProbe {
    public static class Native {
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern IntPtr GetModuleHandleW([MarshalAs(UnmanagedType.LPWStr)] string name);
        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Ansi)]
        static extern IntPtr GetProcAddress(IntPtr module, string name);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern IntPtr GetCurrentThread();

        [DllImport("kernel32.dll", EntryPoint = "SetThreadDescription", ExactSpelling = true, CharSet = CharSet.Unicode)]
        static extern int SetDescription(IntPtr thread, [MarshalAs(UnmanagedType.LPWStr)] string description);
        [DllImport("kernel32.dll", EntryPoint = "GetThreadDescription", ExactSpelling = true)]
        static extern int GetDescription(IntPtr thread, out IntPtr description);
        [DllImport("kernel32.dll")]
        static extern IntPtr LocalFree(IntPtr memory);

        public static string Export(bool set) {
            IntPtr module = GetModuleHandleW("kernel32.dll");
            IntPtr proc = GetProcAddress(module, set ? "SetThreadDescription" : "GetThreadDescription");
            return "kernel32 module=" + module + " export=" + proc;
        }
        public static int Set(string name) { return SetDescription(GetCurrentThread(), name); }
        public static string Get() {
            IntPtr text;
            int hr = GetDescription(GetCurrentThread(), out text);
            if (hr != 0) return "<hr=0x" + hr.ToString("x8") + ">";
            if (text == IntPtr.Zero) return "<null>";
            try { return Marshal.PtrToStringUni(text) ?? "<empty>"; } finally { LocalFree(text); }
        }
    }
}
'@
}
Write-Output ([ThreadNameProbe.Native]::Export($true))
Write-Output ([ThreadNameProbe.Native]::Export($false))
Write-Output ("OS " + [System.Environment]::OSVersion.VersionString)
$hr = [ThreadNameProbe.Native]::Set('wiri-self-check')
Write-Output ("SetThreadDescription hr=0x" + $hr.ToString('x8'))
Write-Output ("GetThreadDescription -> " + [ThreadNameProbe.Native]::Get())
