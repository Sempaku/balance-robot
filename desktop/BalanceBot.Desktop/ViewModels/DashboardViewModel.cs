using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Windows;
using System.Windows.Threading;
using BalanceBot.Core.Telemetry;
using BalanceBot.Core.Tuning;
using BalanceBot.Protocol;
using BalanceBot.Protocol.Packets;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using ScottPlot.Plottables;

namespace BalanceBot.Desktop.ViewModels;

public sealed partial class DashboardViewModel : ObservableObject, IDisposable
{
    private readonly TelemetryHub _telemetryHub;
    private readonly ParameterManager _paramManager;
    private readonly DispatcherTimer _renderTimer;

    // === Стримеры верхнего графика: Ориентация (°) ===
    public DataStreamer? AngleStreamer { get; set; }

    public DataStreamer? AccelAngleStreamer { get; set; }
    public DataStreamer? GyroStreamer { get; set; }
    public DataStreamer? TrimStreamer { get; set; }
    public DataStreamer? LeanStreamer { get; set; }

    // === Стримеры нижнего графика: Моторы и ПИД (об/с) ===
    public DataStreamer? CmdStreamer { get; set; }

    public DataStreamer? WheelRateStreamer { get; set; }
    public DataStreamer? TermPStreamer { get; set; }
    public DataStreamer? TermDStreamer { get; set; }

    public Action? RequestPlotsRefresh { get; set; }

    public Action? RequestPlotRefresh
    {
        get => RequestPlotsRefresh;
        set => RequestPlotsRefresh = value;
    }

    public Action? RequestPlotsReset { get; set; }

    // === Управление тримом на лету ===
    [ObservableProperty] private float _trimOffset;

    // === Видимость каналов верхнего графика ===
    [ObservableProperty] private bool _showAngle = true;

    [ObservableProperty] private bool _showAccelAngle = false;
    [ObservableProperty] private bool _showGyro = true;
    [ObservableProperty] private bool _showTrim = true;
    [ObservableProperty] private bool _showLean = false;

    // === Видимость каналов нижнего графика ===
    [ObservableProperty] private bool _showCmd = true;

    [ObservableProperty] private bool _showWheelRate = true;
    [ObservableProperty] private bool _showTermP = false;
    [ObservableProperty] private bool _showTermD = false;

    // === Управление осциллографом ===
    [ObservableProperty] private bool _isPaused;

    [ObservableProperty] private string _pauseButtonText = "⏸ Пауза";

    // === Метрики и виджеты ===
    [ObservableProperty] private float _angle;

    [ObservableProperty] private float _gyro;
    [ObservableProperty] private float _wheelRateRps;
    [ObservableProperty] private float _cmdRps;
    [ObservableProperty] private float _trim;
    [ObservableProperty] private int _steps;
    [ObservableProperty] private ushort _dtUs;
    [ObservableProperty] private ushort _loopUs;
    [ObservableProperty] private ushort _imuErrors;
    [ObservableProperty] private uint _uptimeSeconds;
    [ObservableProperty] private string _stateText = "ОЖИДАНИЕ (IDLE)";
    [ObservableProperty] private bool _isArmed;

    [ObservableProperty] private bool _isRecording;
    [ObservableProperty] private string _recordingButtonText = "⏺ Запись CSV";

    public DashboardViewModel(TelemetryHub telemetryHub, ParameterManager paramManager)
    {
        _telemetryHub = telemetryHub;
        _paramManager = paramManager;

        // Загружаем сохраненный trim_offset из параметров робота
        if (_paramManager.CurrentValues.TryGetValue("trim_offset", out float savedTrim))
        {
            _trimOffset = savedTrim;
        }

        _renderTimer = new DispatcherTimer(DispatcherPriority.Render)
        {
            Interval = TimeSpan.FromMilliseconds(20)
        };
        _renderTimer.Tick += OnRenderTick;
        _renderTimer.Start();
    }

    protected override void OnPropertyChanged(PropertyChangedEventArgs e)
    {
        base.OnPropertyChanged(e);

        switch (e.PropertyName)
        {
            case nameof(ShowAngle):
                if (AngleStreamer != null) AngleStreamer.IsVisible = ShowAngle;
                break;

            case nameof(ShowAccelAngle):
                if (AccelAngleStreamer != null) AccelAngleStreamer.IsVisible = ShowAccelAngle;
                break;

            case nameof(ShowGyro):
                if (GyroStreamer != null) GyroStreamer.IsVisible = ShowGyro;
                break;

            case nameof(ShowTrim):
                if (TrimStreamer != null) TrimStreamer.IsVisible = ShowTrim;
                break;

            case nameof(ShowLean):
                if (LeanStreamer != null) LeanStreamer.IsVisible = ShowLean;
                break;

            case nameof(ShowCmd):
                if (CmdStreamer != null) CmdStreamer.IsVisible = ShowCmd;
                break;

            case nameof(ShowWheelRate):
                if (WheelRateStreamer != null) WheelRateStreamer.IsVisible = ShowWheelRate;
                break;

            case nameof(ShowTermP):
                if (TermPStreamer != null) TermPStreamer.IsVisible = ShowTermP;
                break;

            case nameof(ShowTermD):
                if (TermDStreamer != null) TermDStreamer.IsVisible = ShowTermD;
                break;

            case nameof(TrimOffset):
                _ = ApplyTrimAsync();
                break;
        }

        RequestPlotsRefresh?.Invoke();
    }

    /// <summary>
    /// Изменение трима на дельту (например, +0.05° или -0.10°).
    /// </summary>
    [RelayCommand]
    public void AdjustTrim(string deltaStr)
    {
        if (float.TryParse(deltaStr, System.Globalization.NumberStyles.Float, System.Globalization.CultureInfo.InvariantCulture, out float delta))
        {
            TrimOffset = (float)Math.Round(Math.Clamp(TrimOffset + delta, -8.0f, 8.0f), 2);
        }
    }

    /// <summary>
    /// Сброс трима в идеальный ноль.
    /// </summary>
    [RelayCommand]
    public void ResetTrim()
    {
        TrimOffset = 0f;
    }

    private async Task ApplyTrimAsync()
    {
        _paramManager.SetLocal("trim_offset", TrimOffset);
        await _paramManager.ApplyModifiedAsync();
    }

    private void OnRenderTick(object? sender, EventArgs e)
    {
        var latest = _telemetryHub.Latest;

        Angle = latest.Angle;
        Gyro = latest.Gyro;
        WheelRateRps = latest.WheelRate / 3200.0f;
        CmdRps = latest.Cmd / 3200.0f;
        Trim = latest.Trim;
        Steps = latest.Steps;
        DtUs = latest.DtUs;
        LoopUs = latest.LoopUs;
        ImuErrors = latest.ImuErrors;
        UptimeSeconds = latest.UptimeMs / 1000;
        IsArmed = (latest.Flags & ProtocolConstants.FlagArmed) != 0;

        StateText = ((byte)latest.State == (byte)RobotState.Balancing)
            ? "БАЛАНС (ACTIVE)"
            : "ОЖИДАНИЕ (IDLE)";

        IsRecording = _telemetryHub.Recorder.IsRecording;
        RecordingButtonText = IsRecording
            ? $"⏹ Стоп ({_telemetryHub.Recorder.RecordsWritten})"
            : "⏺ Запись CSV";

        if (IsPaused) return;

        AngleStreamer?.Add(latest.Angle);
        AccelAngleStreamer?.Add(latest.AccelAngle);
        GyroStreamer?.Add(latest.Gyro);
        TrimStreamer?.Add(latest.Trim);
        LeanStreamer?.Add(latest.TermLean);

        CmdStreamer?.Add(CmdRps);
        WheelRateStreamer?.Add(WheelRateRps);
        TermPStreamer?.Add(latest.TermP / 3200.0f);
        TermDStreamer?.Add(latest.TermD / 3200.0f);

        RequestPlotsRefresh?.Invoke();
    }

    [RelayCommand]
    private void TogglePause()
    {
        IsPaused = !IsPaused;
        PauseButtonText = IsPaused ? "▶ Продолжить" : "⏸ Пауза";
    }

    [RelayCommand]
    private void ResetAxes()
    {
        RequestPlotsReset?.Invoke();
    }

    [RelayCommand]
    private async Task ToggleRecordingAsync()
    {
        if (_telemetryHub.Recorder.IsRecording)
        {
            string? savedPath = _telemetryHub.Recorder.CurrentFilePath;
            ulong count = _telemetryHub.Recorder.RecordsWritten;

            await _telemetryHub.Recorder.StopAsync();
            IsRecording = false;
            RecordingButtonText = "⏺ Запись CSV";

            MessageBox.Show(
                $"Запись сессии успешно завершена!\n\nФайл: {savedPath}\nЗаписано строк: {count}",
                "Запись CSV сохранена",
                MessageBoxButton.OK,
                MessageBoxImage.Information);
        }
        else
        {
            _telemetryHub.Recorder.Start();
            IsRecording = true;
            RecordingButtonText = "⏹ Стоп (0)";
        }
    }

    [RelayCommand]
    private void OpenLogsFolder()
    {
        string dir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "BalanceBot", "Logs");
        Directory.CreateDirectory(dir);

        Process.Start(new ProcessStartInfo
        {
            FileName = dir,
            UseShellExecute = true
        });
    }

    /// <summary>
    /// Останавливает таймер отрисовки, чтобы VM не жила вечно после отключения.
    /// </summary>
    public void Dispose()
    {
        _renderTimer.Stop();
        _renderTimer.Tick -= OnRenderTick;
    }
}