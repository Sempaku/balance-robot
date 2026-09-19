using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using System.Text.RegularExpressions;
using Microsoft.Win32;
using Microsoft.Win32.SafeHandles;

namespace BalanceBot.Core.Input;

/// <summary>
/// Минимальная обёртка над Win32 HID: перечисление устройств по VID/PID,
/// синхронное чтение input-отчётов, запись output-отчётов и feature-запросы.
/// Без внешних зависимостей — только P/Invoke.
/// </summary>
[SupportedOSPlatform("windows")]
internal static class WinHid
{
    private static readonly Guid HidGuid = new("4d1e55b2-f16f-11cf-88cb-001111000030");

    private const uint DigcfPresent = 0x02;
    private const uint DigcfDeviceInterface = 0x01;

    private const uint GenericRead = 0x80000000;
    private const uint GenericWrite = 0x40000000;
    private const uint FileShareRead = 0x00000001;
    private const uint FileShareWrite = 0x00000002;
    private const uint OpenExisting = 3;

    // UsagePage/Usage топ-уровневой коллекции (Generic Desktop / Game Pad)
    public const ushort UsagePageGenericDesktop = 0x01;
    public const ushort UsageGamePad = 0x05;

    [StructLayout(LayoutKind.Sequential)]
    private struct SpDeviceInterfaceData
    {
        public int CbSize;
        public Guid InterfaceClassGuid;
        public uint Flags;
        public IntPtr Reserved;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HidpCaps
    {
        public ushort Usage;
        public ushort UsagePage;
        public ushort InputReportByteLength;
        public ushort OutputReportByteLength;
        public ushort FeatureReportByteLength;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 17)]
        public ushort[] Reserved;
        public ushort NumberLinkCollectionNodes;
        public ushort NumberInputButtonCaps;
        public ushort NumberInputValueCaps;
        public ushort NumberInputDataIndices;
        public ushort NumberOutputButtonCaps;
        public ushort NumberOutputValueCaps;
        public ushort NumberOutputDataIndices;
        public ushort NumberFeatureButtonCaps;
        public ushort NumberFeatureValueCaps;
        public ushort NumberFeatureDataIndices;
    }

    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr SetupDiGetClassDevs(ref Guid classGuid, IntPtr enumerator, IntPtr hwndParent, uint flags);

    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool SetupDiEnumDeviceInterfaces(IntPtr infoSet, IntPtr infoData, ref Guid interfaceGuid, uint index, ref SpDeviceInterfaceData data);

    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool SetupDiGetDeviceInterfaceDetail(IntPtr infoSet, ref SpDeviceInterfaceData data, IntPtr detailData, uint detailSize, ref uint requiredSize, IntPtr deviceInfoData);

    [DllImport("setupapi.dll", SetLastError = true)]
    private static extern bool SetupDiDestroyDeviceInfoList(IntPtr infoSet);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFile(string fileName, uint desiredAccess, uint shareMode, IntPtr securityAttributes, uint creationDisposition, uint flagsAndAttributes, IntPtr templateFile);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool ReadFile(SafeFileHandle file, byte[] buffer, uint bytesToRead, out uint bytesRead, IntPtr overlapped);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool WriteFile(SafeFileHandle file, byte[] buffer, uint bytesToWrite, out uint bytesWritten, IntPtr overlapped);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CancelIoEx(SafeFileHandle file, IntPtr overlapped);

    [DllImport("hid.dll", SetLastError = true)]
    private static extern bool HidD_GetFeature(SafeFileHandle file, byte[] reportBuffer, uint reportBufferLength);

    [DllImport("hid.dll", SetLastError = true)]
    private static extern bool HidD_GetPreparsedData(SafeFileHandle file, out IntPtr preparsedData);

    [DllImport("hid.dll", SetLastError = true)]
    private static extern bool HidP_GetCaps(IntPtr preparsedData, ref HidpCaps caps);

    [DllImport("hid.dll", SetLastError = true)]
    private static extern bool HidD_FreePreparsedData(IntPtr preparsedData);

    // USB:  vid_054c&pid_0ce6
    // BT:    vid&0002054c_pid&0ce6 (0002 — источник Vendor ID, отбрасывается)
    private static readonly Regex VidPidRegex = new(
        @"vid[&_][0-9a-f]*?([0-9a-f]{4})[&_]pid[&_]([0-9a-f]{4})",
        RegexOptions.IgnoreCase | RegexOptions.Compiled);

    /// <summary>
    /// UUID сервиса HID-over-Bluetooth: присутствует в пути интерфейса
    /// BT-геймпада (USB-пути его не содержат).
    /// </summary>
    private const string BluetoothHidServiceUuid = "{00001124-0000-1000-8000-00805f9b34fb}";

    /// <summary>
    /// Путь принадлежит Bluetooth-устройству (а не USB).
    /// </summary>
    public static bool IsBluetoothPath(string path)
    {
        return path.Contains("bthenum", StringComparison.OrdinalIgnoreCase)
            || path.Contains(BluetoothHidServiceUuid, StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>
    /// Все пути HID-интерфейсов в системе: SetupAPI + реестр DeviceClasses.
    ///
    /// Реестр — обход скрытия HidHide (из комплекта DS4Windows/ViGEm):
    /// он убирает интерфейсы из перечисления SetupAPI для посторонних
    /// процессов, но прямое открытие пути при этом работает. Реестр
    /// содержит и устаревшие записи — они отфильтруются при открытии.
    /// </summary>
    public static List<string> EnumerateAllPaths()
    {
        var result = new HashSet<string>(StringComparer.OrdinalIgnoreCase);

        foreach (var path in EnumerateViaSetupApi())
        {
            result.Add(path);
        }

        foreach (var path in EnumerateViaRegistry())
        {
            result.Add(path);
        }

        return result.ToList();
    }

    /// <summary>
    /// Пути HID-устройств с заданным VID/PID (USB и Bluetooth).
    /// </summary>
    public static List<string> EnumeratePaths(ushort vid, ushort pid)
    {
        return EnumerateAllPaths()
            .Where(p =>
            {
                var (v, p2) = ParseVidPid(p);
                return v == vid && p2 == pid;
            })
            .ToList();
    }

    private static IEnumerable<string> EnumerateViaSetupApi()
    {
        var guid = HidGuid;
        IntPtr infoSet = SetupDiGetClassDevs(ref guid, IntPtr.Zero, IntPtr.Zero, DigcfPresent | DigcfDeviceInterface);
        if (infoSet == IntPtr.Zero || infoSet == new IntPtr(-1))
        {
            yield break;
        }

        try
        {
            var data = new SpDeviceInterfaceData { CbSize = Marshal.SizeOf<SpDeviceInterfaceData>() };
            uint index = 0;
            while (SetupDiEnumDeviceInterfaces(infoSet, IntPtr.Zero, ref guid, index++, ref data))
            {
                uint required = 0;
                SetupDiGetDeviceInterfaceDetail(infoSet, ref data, IntPtr.Zero, 0, ref required, IntPtr.Zero);
                if (required == 0)
                {
                    continue;
                }

                IntPtr buffer = Marshal.AllocHGlobal((int)required);
                try
                {
                    int cbSize = IntPtr.Size == 8 ? 8 : 6; // SP_DEVICE_INTERFACE_DETAIL_DATA.cbSize
                    Marshal.WriteInt32(buffer, cbSize);
                    if (SetupDiGetDeviceInterfaceDetail(infoSet, ref data, buffer, required, ref required, IntPtr.Zero))
                    {
                        if (Marshal.PtrToStringUni(buffer + cbSize) is string path)
                        {
                            yield return path;
                        }
                    }
                }
                finally
                {
                    Marshal.FreeHGlobal(buffer);
                }
            }
        }
        finally
        {
            SetupDiDestroyDeviceInfoList(infoSet);
        }
    }

    private static IEnumerable<string> EnumerateViaRegistry()
    {
        using var key = Registry.LocalMachine.OpenSubKey(
            @"SYSTEM\CurrentControlSet\Control\DeviceClasses\{4d1e55b2-f16f-11cf-88cb-001111000030}");

        if (key == null)
        {
            yield break;
        }

        foreach (var name in key.GetSubKeyNames())
        {
            // "##?#HID#..." -> "\\?\HID#..."
            if (name.StartsWith("##?#", StringComparison.Ordinal))
            {
                yield return @"\\?\" + name[4..];
            }
        }
    }

    /// <summary>
    /// VID/PID из пути устройства; (0, 0), если не распознано.
    /// </summary>
    public static (ushort Vid, ushort Pid) ParseVidPid(string path)
    {
        var match = VidPidRegex.Match(path);
        if (!match.Success)
        {
            return (0, 0);
        }

        return (
            Convert.ToUInt16(match.Groups[1].Value, 16),
            Convert.ToUInt16(match.Groups[2].Value, 16));
    }

    /// <summary>
    /// Открытие устройства. При writeAccess=true запрашивается доступ на запись
    /// (нужен для output-отчётов: вибрация, lightbar).
    /// </summary>
    public static SafeFileHandle Open(string path, bool writeAccess)
    {
        uint access = GenericRead | (writeAccess ? GenericWrite : 0u);
        return CreateFile(path, access, FileShareRead | FileShareWrite, IntPtr.Zero, OpenExisting, 0, IntPtr.Zero);
    }

    /// <summary>
    /// Usage топ-уровневой коллекции устройства (например, Game Pad).
    /// false — определить не удалось.
    /// </summary>
    public static bool TryGetUsage(SafeFileHandle file, out ushort usagePage, out ushort usage)
    {
        usagePage = 0;
        usage = 0;

        if (HidD_GetPreparsedData(file, out IntPtr preparsed) && preparsed != IntPtr.Zero)
        {
            try
            {
                var caps = new HidpCaps { Reserved = new ushort[17] };
                if (HidP_GetCaps(preparsed, ref caps))
                {
                    usagePage = caps.UsagePage;
                    usage = caps.Usage;
                    return true;
                }
            }
            finally
            {
                HidD_FreePreparsedData(preparsed);
            }
        }

        return false;
    }

    /// <summary>
    /// Блокирующее чтение одного input-отчёта (report ID в первом байте).
    /// false — устройство отключено или чтение отменено через CancelIoEx.
    /// </summary>
    public static bool Read(SafeFileHandle file, byte[] buffer, out int bytesRead)
    {
        bool ok = ReadFile(file, buffer, (uint)buffer.Length, out uint n, IntPtr.Zero);
        bytesRead = (int)n;
        return ok;
    }

    /// <summary>
    /// Запись output-отчёта (report ID в первом байте).
    /// </summary>
    public static bool Write(SafeFileHandle file, byte[] report)
    {
        return WriteFile(file, report, (uint)report.Length, out _, IntPtr.Zero);
    }

    /// <summary>
    /// Feature-запрос (чтение). buffer[0] = report ID.
    /// </summary>
    public static bool TryGetFeature(SafeFileHandle file, byte reportId, byte[] buffer)
    {
        buffer[0] = reportId;
        Array.Clear(buffer, 1, buffer.Length - 1);
        return HidD_GetFeature(file, buffer, (uint)buffer.Length);
    }

    /// <summary>
    /// Отмена незавершённых операций ввода-вывода (разблокирует Read из другого потока).
    /// </summary>
    public static void CancelIo(SafeFileHandle file)
    {
        try
        {
            CancelIoEx(file, IntPtr.Zero);
        }
        catch (ObjectDisposedException)
        {
        }
    }
}
