using System.Windows.Threading;
using BalanceBot.Core;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace BalanceBot.Desktop.ViewModels;

public sealed partial class ControlViewModel : ObservableObject, IDisposable
{
    private readonly RobotSession _session;
    private readonly DispatcherTimer _driveLoopTimer;

    private bool _keyW;
    private bool _keyS;
    private bool _keyA;
    private bool _keyD;

    [ObservableProperty] private bool _isKeyboardActive;
    [ObservableProperty] private float _speedScale = 0.85f;  // Базовая тяга 85% для уверенного хода
    [ObservableProperty] private float _turnScale = 0.70f;   // Скорость поворота 70%

    [ObservableProperty] private float _currentForward;
    [ObservableProperty] private float _currentTurn;

    public ControlViewModel(RobotSession session)
    {
        _session = session;

        // Отправка команд каждые 28 мс (~35 Гц)
        _driveLoopTimer = new DispatcherTimer(DispatcherPriority.Send)
        {
            Interval = TimeSpan.FromMilliseconds(28)
        };
        _driveLoopTimer.Tick += OnDriveLoopTick;
        _driveLoopTimer.Start();
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
        if (!IsKeyboardActive)
        {
            CurrentForward = 0f;
            CurrentTurn = 0f;
        }

        _session.Drive(CurrentForward, CurrentTurn);
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

    public void Dispose()
    {
        _driveLoopTimer.Stop();
        _session.Drive(0f, 0f);
    }
}