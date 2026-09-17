using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Net.Sockets;
using BalanceBot.Core;
using BalanceBot.Protocol.Packets;
using BalanceBot.Transport.Udp;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace BalanceBot.Desktop.ViewModels;

public sealed partial class MainViewModel : ObservableObject
{
    private CancellationTokenSource? _connectCts;
    private bool _connectCancelled;

    [ObservableProperty] private string _manualIp = "192.168.4.1";
    [ObservableProperty] private bool _isScanning;
    [ObservableProperty] private bool _isConnected;
    [ObservableProperty] private bool _isConnecting;
    [ObservableProperty] private string _connectionStatus = "Не подключено";
    [ObservableProperty] private RobotInfo? _selectedRobot;

    [ObservableProperty] private DashboardViewModel? _dashboard;
    [ObservableProperty] private TuningViewModel? _tuning;
    [ObservableProperty] private ControlViewModel? _control;
    [ObservableProperty] private WifiViewModel? _wifi;
    [ObservableProperty] private BlackBoxViewModel? _blackBox;

    public bool CanConnect => !IsConnected && !IsConnecting;
    public bool CanEditInputs => !IsConnected && !IsConnecting;

    public ObservableCollection<RobotInfo> DiscoveredRobots { get; } = new();

    public RobotSession? CurrentSession { get; private set; }

    public MainViewModel()
    {
    }

    protected override void OnPropertyChanged(PropertyChangedEventArgs e)
    {
        base.OnPropertyChanged(e);

        if (e.PropertyName is nameof(IsConnected) or nameof(IsConnecting))
        {
            OnPropertyChanged(nameof(CanConnect));
            OnPropertyChanged(nameof(CanEditInputs));
        }

        if (e.PropertyName == nameof(SelectedRobot) && SelectedRobot != null)
        {
            if (!string.IsNullOrWhiteSpace(SelectedRobot.RemoteIpAddress))
            {
                ManualIp = SelectedRobot.RemoteIpAddress;
            }
        }
    }

    [RelayCommand]
    public async Task ScanNetworkAsync()
    {
        IsScanning = true;
        ConnectionStatus = "Поиск роботов в Wi-Fi сети...";
        DiscoveredRobots.Clear();

        try
        {
            var list = await UdpDiscoveryService.DiscoverRobotsAsync(TimeSpan.FromSeconds(1.5));
            foreach (var r in list)
            {
                DiscoveredRobots.Add(r);
            }

            if (DiscoveredRobots.Count > 0)
            {
                SelectedRobot = DiscoveredRobots[0];
                ConnectionStatus = $"Найдено роботов: {DiscoveredRobots.Count}";
            }
            else
            {
                ConnectionStatus = "Роботы не найдены. Проверь Wi-Fi.";
            }
        }
        finally
        {
            IsScanning = false;
        }
    }

    [RelayCommand]
    public async Task ConnectAsync()
    {
        string ip = SelectedRobot?.RemoteIpAddress ?? ManualIp.Trim();
        int port = SelectedRobot?.CommandPort ?? 5556;
        string name = SelectedRobot?.Name ?? "BalanceBot";

        IsConnecting = true;
        ConnectionStatus = $"Подключение к {ip}...";
        _connectCancelled = false;

        _connectCts?.Dispose();
        _connectCts = new CancellationTokenSource(TimeSpan.FromSeconds(3.5));

        try
        {
            if (CurrentSession != null)
            {
                Dashboard?.Dispose();
                await CurrentSession.DisposeAsync();
            }

            var info = new RobotInfo(name, "1.0", port, ip);
            CurrentSession = await RobotSession.ConnectAsync(info, _connectCts.Token);

            Dashboard = new DashboardViewModel(CurrentSession.Telemetry, CurrentSession.Parameters);
            Tuning = new TuningViewModel(CurrentSession.Parameters);
            Control = new ControlViewModel(CurrentSession);
            Wifi = new WifiViewModel(CurrentSession);
            BlackBox = new BlackBoxViewModel(CurrentSession);

            IsConnected = true;
            ConnectionStatus = $"Подключено к {name} ({ip})";
        }
        catch (OperationCanceledException)
        {
            IsConnected = false;
            ConnectionStatus = _connectCancelled
                ? "Подключение отменено"
                : $"Таймаут: {ip} не отвечает (проверь IP или Wi-Fi)";
        }
        catch (SocketException sockEx)
        {
            IsConnected = false;
            ConnectionStatus = sockEx.SocketErrorCode switch
            {
                SocketError.ConnectionRefused => $"Робот доступен, но порт {port} закрыт",
                SocketError.HostUnreachable => "Хост недостижим (проверь Wi-Fi сеть)",
                SocketError.NetworkUnreachable => "Сеть недостижима",
                SocketError.AddressNotAvailable => "Неверный IP-адрес робота",
                _ => $"Робот не найден по адресу {ip}"
            };
        }
        catch (Exception ex)
        {
            IsConnected = false;
            ConnectionStatus = $"Ошибка связи: {ex.Message}";
        }
        finally
        {
            IsConnecting = false;
        }
    }

    [RelayCommand]
    public void CancelConnect()
    {
        if (IsConnecting)
        {
            _connectCancelled = true;
            _connectCts?.Cancel();
            ConnectionStatus = "Подключение отменено";
            IsConnecting = false;
        }
    }

    [RelayCommand]
    public async Task DisconnectAsync()
    {
        Control?.Dispose();
        Dashboard?.Dispose();

        if (CurrentSession != null)
        {
            await CurrentSession.DisposeAsync();
            CurrentSession = null;
        }

        IsConnected = false;
        Dashboard = null;
        Tuning = null;
        Control = null;
        Wifi = null;
        BlackBox = null;
        ConnectionStatus = "Отключено";
    }

    [RelayCommand]
    public async Task ArmRobotAsync()
    {
        if (CurrentSession != null)
        {
            try
            {
                await CurrentSession.ArmAsync();
            }
            catch (Exception ex)
            {
                ConnectionStatus = $"Ошибка включения: {ex.Message}";
            }
        }
    }

    [RelayCommand]
    public async Task DisarmRobotAsync()
    {
        if (CurrentSession != null)
        {
            try
            {
                CurrentSession.Drive(0f, 0f);
                await CurrentSession.DisarmAsync();
            }
            catch (Exception ex)
            {
                ConnectionStatus = $"Стоп выполнен (связь TCP: {ex.Message})";
            }
        }
    }
}