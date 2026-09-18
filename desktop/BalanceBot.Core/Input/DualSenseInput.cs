using Microsoft.Win32.SafeHandles;

namespace BalanceBot.Core.Input;

public enum DualSenseTransport
{
    Unknown,
    Usb,
    Bluetooth,
}

[Flags]
public enum DualSenseButtons
{
    None = 0,
    Square = 1 << 0,
    Cross = 1 << 1,
    Circle = 1 << 2,
    Triangle = 1 << 3,
    L1 = 1 << 4,
    R1 = 1 << 5,
    Create = 1 << 6,
    Options = 1 << 7,
    L3 = 1 << 8,
    R3 = 1 << 9,
    Ps = 1 << 10,
    Mute = 1 << 11,
    Touchpad = 1 << 12,
    DpadUp = 1 << 13,
    DpadDown = 1 << 14,
    DpadLeft = 1 << 15,
    DpadRight = 1 << 16,
}

/// <summary>
/// Состояние геймпада: стики -1..1 (вверх = +1), триггеры 0..1.
/// </summary>
public readonly record struct DualSenseState(
    float LeftStickX,
    float LeftStickY,
    float RightStickX,
    float RightStickY,
    float L2,
    float R2,
    DualSenseButtons Buttons);

/// <summary>
/// Чтение геймпада DualSense через raw HID (без внешних библиотек).
///
/// Поддерживаются оба транспорта:
/// - USB: input-отчёт 0x01 (64 байта), output-отчёт 0x02 (48 байт);
/// - Bluetooth: input-отчёт 0x31 (78 байт, enhanced) или 0x01 (simple),
///   output-отчёт 0x31 (78 байт) с CRC32 в конце.
///
/// Раскладки отчётов — из SDL2 (src/joystick/hidapi/SDL_hidapi_ps5.c).
/// Для BT feature-запрос 0x09 переключает геймпад в enhanced-режим.
/// </summary>
public sealed class DualSenseInput : IDisposable
{
    private const ushort VendorId = 0x054C;
    private static readonly ushort[] ProductIds = { 0x0CE6, 0x0DF2 }; // DualSense, DualSense Edge

    private const byte ReportState = 0x01;     // USB (64 байта) и BT simple
    private const byte ReportUsbEffects = 0x02;
    private const byte ReportBtEffects = 0x31; // BT: input (enhanced) и output

    private const int UsbOutputSize = 48;
    private const int BtOutputSize = 78;

    // Смещения полей внутри effects-структуры (DS5EffectsState_t из SDL2)
    private const int EffEnableBits1 = 0;
    private const int EffEnableBits2 = 1;
    private const int EffRumbleRight = 2;
    private const int EffRumbleLeft = 3;
    private const int EffEnableBits3 = 38;
    private const int EffLedRed = 44;
    private const int EffLedGreen = 45;
    private const int EffLedBlue = 46;

    private readonly object _sync = new();
    private readonly CancellationTokenSource _cts = new();
    private Thread? _thread;
    private Timer? _keepAlive;
    private SafeFileHandle? _handle;
    private DualSenseState _state;
    private DualSenseButtons _prevButtons;

    private byte _rumbleLeft;
    private byte _rumbleRight;
    private byte _ledR;
    private byte _ledG;
    private byte _ledB;
    private bool _ledResetDone;
    private DateTime _ledReadyAt;

    public bool IsConnected { get; private set; }

    public DualSenseTransport Transport { get; private set; }

    /// <summary>Возможность output-отчётов (вибрация, lightbar).</summary>
    public bool EffectsSupported { get; private set; }

    public DualSenseState State
    {
        get { lock (_sync) { return _state; } }
        private set { lock (_sync) { _state = value; } }
    }

    /// <summary>Геймпад подключён (USB или BT).</summary>
    public event Action? Connected;

    /// <summary>Геймпад отключён.</summary>
    public event Action? Disconnected;

    /// <summary>Нажатие кнопок (только фронт: новые нажатые с прошлого отчёта).</summary>
    public event Action<DualSenseButtons>? ButtonPressed;

    /// <summary>
    /// Запуск фонового потока: поиск устройства, чтение отчётов,
    /// автопереподключение при отключении.
    /// </summary>
    public void Start()
    {
        if (_thread != null)
        {
            return;
        }

        _thread = new Thread(RunLoop)
        {
            IsBackground = true,
            Name = "dualsense-hid",
        };
        _thread.Start();
    }

    /// <summary>
    /// Вибрация (0..255) и цвет lightbar. Ничего не делает без подключения.
    /// </summary>
    public void SetFeedback(byte rumbleLeft, byte rumbleRight, byte ledR, byte ledG, byte ledB)
    {
        lock (_sync)
        {
            _rumbleLeft = rumbleLeft;
            _rumbleRight = rumbleRight;
            _ledR = ledR;
            _ledG = ledG;
            _ledB = ledB;
        }

        SendEffects();
    }

    public void Dispose()
    {
        // Гасим вибрацию и lightbar перед закрытием
        if (IsConnected && EffectsSupported)
        {
            try
            {
                lock (_sync)
                {
                    _rumbleLeft = 0;
                    _rumbleRight = 0;
                    _ledR = 0;
                    _ledG = 0;
                    _ledB = 0;
                }

                SendEffects();
            }
            catch
            {
                // устройство уже могло пропасть
            }
        }

        _cts.Cancel();
        SafeFileHandle? handle;
        lock (_sync)
        {
            handle = _handle;
        }

        if (handle != null)
        {
            WinHid.CancelIo(handle);
        }

        _thread?.Join(500);
        _keepAlive?.Dispose();
        _cts.Dispose();
    }

    private void RunLoop()
    {
        while (!_cts.IsCancellationRequested)
        {
            try
            {
                ConnectOnce();
            }
            catch
            {
                // устройство пропало или не найдено — повторим через секунду
            }

            if (!_cts.IsCancellationRequested)
            {
                Thread.Sleep(1000);
            }
        }
    }

    private void ConnectOnce()
    {
        string? path = FindDevicePath();
        if (path == null)
        {
            throw new IOException("DualSense не найден");
        }

        bool isBt = path.Contains("bthenum", StringComparison.OrdinalIgnoreCase);
        Transport = isBt ? DualSenseTransport.Bluetooth : DualSenseTransport.Usb;

        var handle = WinHid.Open(path, writeAccess: true);
        EffectsSupported = !handle.IsInvalid;
        if (handle.IsInvalid)
        {
            handle.Dispose();
            handle = WinHid.Open(path, writeAccess: false);
            if (handle.IsInvalid)
            {
                handle.Dispose();
                throw new IOException("HID-устройство занято");
            }
        }

        lock (_sync)
        {
            _handle = handle;
        }

        _prevButtons = DualSenseButtons.None;
        _ledResetDone = false;

        // На BT анимация подключения занимает ~3.4 с: lightbar раньше времени
        // не принимает цвет (SDL ждёт метку времени сенсоров 10.2M × 0.33 мкс)
        _ledReadyAt = DateTime.UtcNow.AddSeconds(isBt ? 5.0 : 0.0);

        try
        {
            if (isBt)
            {
                // Feature-запрос 0x09 (серийный номер) переключает BT-геймпад
                // в enhanced-режим: input 0x31 + приём output-отчётов
                WinHid.TryGetFeature(handle, 0x09, new byte[64]);
            }

            IsConnected = true;

            // BT-соединение требует периодических output-отчётов,
            // иначе геймпад отключается — шлём текущие эффекты каждые 500 мс
            if (isBt && EffectsSupported)
            {
                _keepAlive = new Timer(
                    _ => { if (IsConnected) { SendEffects(); } },
                    null,
                    500,
                    500);
            }

            Connected?.Invoke();
            if (EffectsSupported)
            {
                SendEffects();
            }

            ReadLoop(handle);
        }
        finally
        {
            IsConnected = false;
            _keepAlive?.Dispose();
            _keepAlive = null;
            lock (_sync)
            {
                _handle = null;
            }

            WinHid.CancelIo(handle);
            handle.Dispose();
            Disconnected?.Invoke();
        }
    }

    private static string? FindDevicePath()
    {
        foreach (var pid in ProductIds)
        {
            var paths = WinHid.EnumeratePaths(VendorId, pid);

            // USB предпочтительнее: стабильнее, без CRC и keep-alive
            var usb = paths.FirstOrDefault(p =>
                !p.Contains("bthenum", StringComparison.OrdinalIgnoreCase));
            if (usb != null)
            {
                return usb;
            }

            if (paths.Count > 0)
            {
                return paths[0];
            }
        }

        return null;
    }

    private void ReadLoop(SafeFileHandle handle)
    {
        var buffer = new byte[128];
        while (!_cts.IsCancellationRequested)
        {
            if (!WinHid.Read(handle, buffer, out int read) || read <= 0)
            {
                break; // устройство отключено или чтение отменено
            }

            ParseReport(buffer, read);
        }
    }

    private void ParseReport(byte[] d, int len)
    {
        // Раскладки из SDL2 (SDL_hidapi_ps5.c), смещения от начала отчёта:
        // USB  0x01 (64 байта): стики @1..4, триггеры @5..6, кнопки @8..10
        // BT   0x31 (78 байт):  стики @3..6, триггеры @7..8, кнопки @10..12
        // BT   0x01 (simple):   стики @1..4, кнопки @5..7,   триггеры @8..9
        int sticks, triggers, buttons;
        if (d[0] == ReportBtEffects && len >= 13)
        {
            sticks = 3;
            triggers = 7;
            buttons = 10;
        }
        else if (d[0] == ReportState && Transport == DualSenseTransport.Usb && len >= 11)
        {
            sticks = 1;
            triggers = 5;
            buttons = 8;
        }
        else if (d[0] == ReportState && len >= 10)
        {
            sticks = 1;
            triggers = 8;
            buttons = 5;
        }
        else
        {
            return;
        }

        var flags = DecodeButtons(d[buttons], d[buttons + 1], d[buttons + 2]);

        State = new DualSenseState(
            NormalizeAxis(d[sticks]),
            -NormalizeAxis(d[sticks + 1]), // 0 = вверх → инвертируем: вверх = +1
            NormalizeAxis(d[sticks + 2]),
            -NormalizeAxis(d[sticks + 3]),
            d[triggers] / 255f,
            d[triggers + 1] / 255f,
            flags);

        var pressed = flags & ~_prevButtons;
        _prevButtons = flags;
        if (pressed != DualSenseButtons.None)
        {
            ButtonPressed?.Invoke(pressed);
        }
    }

    private static DualSenseButtons DecodeButtons(byte b0, byte b1, byte b2)
    {
        var result = DualSenseButtons.None;

        // b0: старший полубайт — квадрат/крест/круг/треугольник,
        // младший — D-pad (hat: 0=N .. 7=NW, 8=отпущен)
        if ((b0 & 0x10) != 0) result |= DualSenseButtons.Square;
        if ((b0 & 0x20) != 0) result |= DualSenseButtons.Cross;
        if ((b0 & 0x40) != 0) result |= DualSenseButtons.Circle;
        if ((b0 & 0x80) != 0) result |= DualSenseButtons.Triangle;

        switch (b0 & 0x0F)
        {
            case 0: result |= DualSenseButtons.DpadUp; break;
            case 1: result |= DualSenseButtons.DpadUp | DualSenseButtons.DpadRight; break;
            case 2: result |= DualSenseButtons.DpadRight; break;
            case 3: result |= DualSenseButtons.DpadRight | DualSenseButtons.DpadDown; break;
            case 4: result |= DualSenseButtons.DpadDown; break;
            case 5: result |= DualSenseButtons.DpadDown | DualSenseButtons.DpadLeft; break;
            case 6: result |= DualSenseButtons.DpadLeft; break;
            case 7: result |= DualSenseButtons.DpadUp | DualSenseButtons.DpadLeft; break;
        }

        // b1: L1, R1, L2/R2 (цифровые), create, options, L3, R3
        if ((b1 & 0x01) != 0) result |= DualSenseButtons.L1;
        if ((b1 & 0x02) != 0) result |= DualSenseButtons.R1;
        if ((b1 & 0x10) != 0) result |= DualSenseButtons.Create;
        if ((b1 & 0x20) != 0) result |= DualSenseButtons.Options;
        if ((b1 & 0x40) != 0) result |= DualSenseButtons.L3;
        if ((b1 & 0x80) != 0) result |= DualSenseButtons.R3;

        // b2: PS, mute, тачпад
        if ((b2 & 0x01) != 0) result |= DualSenseButtons.Ps;
        if ((b2 & 0x02) != 0) result |= DualSenseButtons.Mute;
        if ((b2 & 0x04) != 0) result |= DualSenseButtons.Touchpad;

        return result;
    }

    private static float NormalizeAxis(byte v)
    {
        return (v - 127.5f) / 127.5f;
    }

    private void SendEffects()
    {
        byte[] report;
        SafeFileHandle? handle;
        lock (_sync)
        {
            handle = _handle;
            if (!IsConnected || handle == null || handle.IsInvalid || !EffectsSupported)
            {
                return;
            }

            report = BuildReportLocked();
        }

        WinHid.Write(handle, report);
    }

    /// <summary>Сборка output-отчёта. Вызывается только под блокировкой _sync.</summary>
    private byte[] BuildReportLocked()
    {
        byte[] data;
        int e;
        if (Transport == DualSenseTransport.Bluetooth)
        {
            data = new byte[BtOutputSize];
            data[0] = ReportBtEffects;
            data[1] = 0x02; // магическое значение из SDL2
            e = 2;
        }
        else
        {
            data = new byte[UsbOutputSize];
            data[0] = ReportUsbEffects;
            e = 1;
        }

        if (_rumbleLeft > 0 || _rumbleRight > 0)
        {
            data[e + EffEnableBits1] |= 0x02; // отключаем аудио-тактильность
            data[e + EffEnableBits3] |= 0x04; // улучшенная вибрация (прошивка >= 2.24)
            data[e + EffRumbleRight] = _rumbleRight;
            data[e + EffRumbleLeft] = _rumbleLeft;
        }

        if (DateTime.UtcNow >= _ledReadyAt)
        {
            if (!_ledResetDone)
            {
                // Сброс анимации подключения — отдельным отчётом, как в SDL2
                data[e + EffEnableBits2] |= 0x08;
                _ledResetDone = true;
                return data;
            }

            data[e + EffEnableBits2] |= 0x04; // цвет lightbar
            data[e + EffLedRed] = _ledR;
            data[e + EffLedGreen] = _ledG;
            data[e + EffLedBlue] = _ledB;
        }

        if (Transport == DualSenseTransport.Bluetooth)
        {
            // BT-отчёт требует CRC32 в конце: заголовок 0xA2 + первые 74 байта
            uint crc = Crc32Update(0, [0xA2]);
            crc = Crc32Update(crc, data.AsSpan(0, BtOutputSize - 4));
            data[74] = (byte)crc;
            data[75] = (byte)(crc >> 8);
            data[76] = (byte)(crc >> 16);
            data[77] = (byte)(crc >> 24);
        }

        return data;
    }

    private static readonly uint[] CrcTable = BuildCrcTable();

    private static uint[] BuildCrcTable()
    {
        var table = new uint[256];
        for (uint i = 0; i < 256; i++)
        {
            uint c = i;
            for (int k = 0; k < 8; k++)
            {
                c = (c & 1) != 0 ? 0xEDB88320u ^ (c >> 1) : c >> 1;
            }

            table[i] = c;
        }

        return table;
    }

    /// <summary>CRC-32 (zlib-совместимый, полином 0xEDB88320).</summary>
    private static uint Crc32Update(uint crc, ReadOnlySpan<byte> data)
    {
        crc ^= 0xFFFFFFFFu;
        for (int i = 0; i < data.Length; i++)
        {
            crc = CrcTable[(crc ^ data[i]) & 0xFF] ^ (crc >> 8);
        }

        return crc ^ 0xFFFFFFFFu;
    }
}
