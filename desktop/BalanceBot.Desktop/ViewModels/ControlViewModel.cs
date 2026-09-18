using System.Collections.ObjectModel;
using System.Windows.Threading;
using BalanceBot.Core;
using BalanceBot.Core.Input;
using BalanceBot.Protocol.Packets;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace BalanceBot.Desktop.ViewModels;

public sealed partial class ControlViewModel : ObservableObject, IDisposable
{
    private const byte StateBalancing = 1; // STATE_BALANCING в прошивке
    private const float StickDeadzone = 0.08f;
    private const float TrimStepDeg = 0.1f;
    private const int GamepadLogLimit = 60;

    private readonly RobotSession _session;
    private readonly DispatcherTimer _driveLoopTimer;
    private readonly DualSenseInput _gamepad = new();
    private readonly Dispatcher _dispatcher;

    private bool _keyW;
    private bool _keyS;
    private bool _keyA;
    private bool _keyD;

    [ObservableProperty] private bool _isKeyboardActive;
    [ObservableProperty] private float _speedScale = 0.85f;  // Базовая тяга 85% для уверенного хода
    [ObservableProperty] private float _turnScale = 0.70f;   // Скорость поворота 70%

    [ObservableProperty] private float _currentForward;
    [ObservableProperty] private float _currentTurn;

    [ObservableProperty] private bool _isGamepadConnected;
    [ObservableProperty] private bool _isGamepadActive = true;
    [ObservableProperty] private string _gamepadStatusText = "Не найден";

    /// <summary>Устройства из последнего поиска HID.</summary>
    public ObservableCollection<GamepadCandidate> GamepadDevices { get; } = new();

    /// <summary>Журнал подключения геймпада.</summary>
    public ObservableCollection<string> GamepadLog { get; } = new();

    // Телеметрия для обратной связи на геймпад (фаза 2)
    private volatile float _teleAngle;
    private volatile float _teleTrim;
    private volatile byte _teleState;
    private volatile ushort _teleImuErrors;
    private DateTime _teleLastAt = DateTime.MinValue;
    private bool _wasBalancing;
    private DateTime _fallPulseUntil = DateTime.MinValue;
    private byte _fbRumble = 255; // не равно нулю — заставит отправить первый отчёт
    private byte _fbLedR = 255;
    private byte _fbLedG = 255;
    private byte _fbLedB = 255;

    public ControlViewModel(RobotSession session)
    {
        _session = session;
        _dispatcher = Dispatcher.CurrentDispatcher;

        // Отправка команд каждые 28 мс (~35 Гц)
        _driveLoopTimer = new DispatcherTimer(DispatcherPriority.Send)
        {
            Interval = TimeSpan.FromMilliseconds(28)
        };
        _driveLoopTimer.Tick += OnDriveLoopTick;
        _driveLoopTimer.Start();

        // Геймпад DualSense: стики → ход, кнопки → arm/disarm/трим
        _gamepad.Connected += OnGamepadConnected;
        _gamepad.Disconnected += OnGamepadDisconnected;
        _gamepad.ButtonPressed += OnGamepadButton;
        _gamepad.Log += OnGamepadLog;
        _gamepad.Start();

        // Телеметрия → вибрация и lightbar (фаза 2)
        _session.Telemetry.OnPacketReceived += OnTelemetryPacket;
    }

    public void UpdateKey(bool isDown, bool w, bool s, bool a, bool d)
    {
        if (w) _keyW = isDown;
        if (s) _keyS = isDown;
        if (a) _keyA = isDown;
        if (d) _keyD = isDown;

        RecalculateVectors();
    }

    private void RecalculateVectors()
    {
        // Геймпад активен — клавиатура не перебивает стики
        if (_gamepad.IsConnected && IsGamepadActive)
        {
            return;
        }

        if (!IsKeyboardActive)
        {
            CurrentForward = 0f;
            CurrentTurn = 0f;
            return;
        }

        float fwd = 0f;
        if (_keyW) fwd += 1.0f;
        if (_keyS) fwd -= 1.0f;

        float trn = 0f;
        if (_keyD) trn += 1.0f;
        if (_keyA) trn -= 1.0f;

        CurrentForward = fwd * SpeedScale;
        CurrentTurn = trn * TurnScale;
    }

    private void OnDriveLoopTick(object? sender, EventArgs e)
    {
        if (_gamepad.IsConnected && IsGamepadActive)
        {
            var s = _gamepad.State;

            // R2 — форсаж: 35% тяги без нажатия, 100% при полном нажатии
            float boost = 0.35f + 0.65f * s.R2;
            CurrentForward = Deadzone(s.LeftStickY) * SpeedScale * boost;
            CurrentTurn = Deadzone(s.RightStickX) * TurnScale;
        }
        else if (!IsKeyboardActive)
        {
            CurrentForward = 0f;
            CurrentTurn = 0f;
        }

        _session.Drive(CurrentForward, CurrentTurn);
        UpdateGamepadFeedback();
    }

    private static float Deadzone(float v)
    {
        float a = Math.Abs(v);
        if (a < StickDeadzone)
        {
            return 0f;
        }

        return Math.Sign(v) * (a - StickDeadzone) / (1f - StickDeadzone);
    }

    [RelayCommand]
    private void EmergencyStop()
    {
        _keyW = _keyS = _keyA = _keyD = false;
        CurrentForward = 0f;
        CurrentTurn = 0f;
        _session.Drive(0f, 0f);
        _ = _session.DisarmAsync();
    }

    // =========================================================================
    // Геймпад DualSense
    // =========================================================================

    private void OnGamepadConnected()
    {
        IsGamepadConnected = true;
        GamepadStatusText = _gamepad.Transport == DualSenseTransport.Usb
            ? "USB подключён"
            : "Bluetooth подключён";
    }

    private void OnGamepadDisconnected()
    {
        IsGamepadConnected = false;
        GamepadStatusText = _gamepad.ManualPath != null
            ? "Устройство отключено (ручной выбор)"
            : "Не найден";

        // Геймпад пропал — немедленно глушим ход; дальше страхует
        // прошивочный watchdog 300 мс
        if (IsGamepadActive)
        {
            CurrentForward = 0f;
            CurrentTurn = 0f;
            _session.Drive(0f, 0f);
        }
    }

    private void OnGamepadLog(string line)
    {
        void Add()
        {
            GamepadLog.Add(line);
            while (GamepadLog.Count > GamepadLogLimit)
            {
                GamepadLog.RemoveAt(0);
            }
        }

        if (_dispatcher.CheckAccess())
        {
            Add();
        }
        else
        {
            _dispatcher.BeginInvoke(Add);
        }
    }

    /// <summary>Поиск HID-устройств для панели геймпада.</summary>
    [RelayCommand]
    private async Task RescanGamepadsAsync()
    {
        GamepadStatusText = "Сканирую HID...";
        var devices = await Task.Run(DualSenseInput.Scan);

        GamepadDevices.Clear();
        foreach (var device in devices)
        {
            GamepadDevices.Add(device);
        }

        GamepadStatusText = devices.Count == 0
            ? "Геймпады не найдены"
            : $"Найдено: {devices.Count}";
        OnGamepadLog($"{DateTime.Now:HH:mm:ss} Поиск: найдено устройств — {devices.Count}");
    }

    /// <summary>Ручное подключение к выбранному устройству из списка.</summary>
    [RelayCommand]
    private void ConnectGamepad(string path)
    {
        _gamepad.ConnectTo(path);
    }

    /// <summary>Возврат к автопоиску DualSense.</summary>
    [RelayCommand]
    private void UseAutoGamepad()
    {
        _gamepad.UseAuto();
    }

    /// <summary>Тест вибрации и lightbar на 600 мс.</summary>
    [RelayCommand]
    private void TestRumble()
    {
        _gamepad.SetFeedback(220, 220, 0, 60, 0);
        _fbRumble = 255; // заставит цикл обратной связи вернуть нормальное состояние
    }

    private void OnGamepadButton(DualSenseButtons buttons)
    {
        // PS — аварийный стоп работает всегда, даже при выключенном геймпад-управлении
        if ((buttons & DualSenseButtons.Ps) != 0)
        {
            EmergencyStop();
            return;
        }

        if (!IsGamepadActive)
        {
            return;
        }

        if ((buttons & DualSenseButtons.Cross) != 0)
        {
            RunAsync(() => _session.ArmAsync());
        }
        else if ((buttons & DualSenseButtons.Circle) != 0)
        {
            RunAsync(() => _session.DisarmAsync());
        }
        else if ((buttons & DualSenseButtons.Create) != 0)
        {
            IsGamepadActive = !IsGamepadActive;
        }
        else if ((buttons & DualSenseButtons.DpadUp) != 0)
        {
            RunAsync(() => NudgeTrimAsync(+TrimStepDeg));
        }
        else if ((buttons & DualSenseButtons.DpadDown) != 0)
        {
            RunAsync(() => NudgeTrimAsync(-TrimStepDeg));
        }
    }

    private static async void RunAsync(Func<Task> action)
    {
        try
        {
            await action();
        }
        catch
        {
            // сетевые ошибки кнопок не роняют UI
        }
    }

    /// <summary>
    /// Шаг трима с D-pad: set_params + save_params одним пакетом (batch).
    /// </summary>
    private async Task NudgeTrimAsync(float delta)
    {
        _session.Parameters.CurrentValues.TryGetValue("trim_offset", out float current);
        float target = Math.Clamp(current + delta, -8f, 8f);

        bool ok = await _session.Tcp.SetAndSaveParamsAsync(
            new Dictionary<string, float> { ["trim_offset"] = target });

        if (ok)
        {
            _session.Parameters.CurrentValues["trim_offset"] = target;
        }
    }

    // =========================================================================
    // Обратная связь: телеметрия → вибрация и lightbar (фаза 2)
    // =========================================================================

    private void OnTelemetryPacket(TelemetryPacket packet)
    {
        _teleAngle = packet.Angle;
        _teleTrim = packet.Trim;
        _teleState = packet.State;
        _teleImuErrors = packet.ImuErrors;
        _teleLastAt = DateTime.UtcNow;
    }

    private void UpdateGamepadFeedback()
    {
        if (!_gamepad.IsConnected || !_gamepad.EffectsSupported)
        {
            return;
        }

        byte rumble = 0;
        byte ledR = 0, ledG = 0, ledB = 0;

        bool telemetryFresh = DateTime.UtcNow - _teleLastAt < TimeSpan.FromSeconds(1.5);
        bool balancing = telemetryFresh && _teleState == StateBalancing;

        // Lightbar: зелёный — баланс, тускло-синий — ожидание,
        // серый — телеметрия пропала
        if (!telemetryFresh)
        {
            ledR = 20;
            ledG = 20;
            ledB = 20;
        }
        else if (balancing)
        {
            ledG = 40;
        }
        else
        {
            ledB = 40;
        }

        // Падение или дизарм: сильный импульс вибрации
        if (_wasBalancing && !balancing)
        {
            _fallPulseUntil = DateTime.UtcNow.AddMilliseconds(700);
        }

        _wasBalancing = balancing;
        if (DateTime.UtcNow < _fallPulseUntil)
        {
            rumble = 255;
        }

        // Приближение к порогу падения: вибрация нарастает,
        // lightbar краснеет (порог 0.6×fall — как предупреждение детектора)
        if (telemetryFresh)
        {
            _session.Parameters.CurrentValues.TryGetValue("fall", out float fall);
            float warn = Math.Abs(_teleAngle - _teleTrim) / Math.Max(1f, fall * 0.6f);
            if (warn > 0.4f)
            {
                float clamped = Math.Clamp((warn - 0.4f) / 0.6f, 0f, 1f);
                rumble = Math.Max(rumble, (byte)(clamped * 160));
            }

            if (warn > 0.5f)
            {
                ledR = 40;
                ledG = 0;
                ledB = 0;
            }
        }

        // Ошибки IMU — максимальная вибрация
        if (_teleImuErrors > 0)
        {
            rumble = 255;
            ledR = 40;
            ledB = 40;
        }

        if (rumble != _fbRumble || ledR != _fbLedR || ledG != _fbLedG || ledB != _fbLedB)
        {
            _fbRumble = rumble;
            _fbLedR = ledR;
            _fbLedG = ledG;
            _fbLedB = ledB;
            _gamepad.SetFeedback(rumble, rumble, ledR, ledG, ledB);
        }
    }

    public void Dispose()
    {
        _driveLoopTimer.Stop();
        _session.Telemetry.OnPacketReceived -= OnTelemetryPacket;
        _session.Drive(0f, 0f);
        _gamepad.Dispose();
    }
}
