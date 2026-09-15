using System.Collections.ObjectModel;
using BalanceBot.Core;
using BalanceBot.Protocol.Dtos;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace BalanceBot.Desktop.ViewModels;

public sealed partial class WifiViewModel : ObservableObject
{
    private readonly RobotSession _session;

    public ObservableCollection<WifiNetworkDto> Networks { get; } = new();

    // Текущий статус Wi-Fi робота
    [ObservableProperty] private string _currentMode = "Определение...";

    [ObservableProperty] private string _currentSsid = "Не подключено";
    [ObservableProperty] private string _currentIp = "0.0.0.0";
    [ObservableProperty] private bool _isStationMode;

    [ObservableProperty] private string _newSsid = "";
    [ObservableProperty] private string _newPassword = "";
    [ObservableProperty] private int _newPriority = 10;
    [ObservableProperty] private string _statusMessage = "Загрузка данных Wi-Fi...";
    [ObservableProperty] private bool _isBusy;

    public WifiViewModel(RobotSession session)
    {
        _session = session;
        _ = RefreshListAsync();
    }

    [RelayCommand]
    public async Task RefreshListAsync()
    {
        IsBusy = true;
        StatusMessage = "Чтение сетей и статуса из робота...";
        try
        {
            // 1. Читаем текущее системное состояние (режим, активный SSID, IP)
            var sysInfo = await _session.Tcp.GetSystemInfoAsync();
            if (sysInfo != null)
            {
                CurrentSsid = string.IsNullOrWhiteSpace(sysInfo.Ssid) ? "Без имени" : sysInfo.Ssid;
                CurrentIp = sysInfo.Ip;
                IsStationMode = sysInfo.Mode == "station";
                CurrentMode = IsStationMode
                    ? "Клиент роутера (Station)"
                    : "Собственная точка (SoftAP Portal)";
            }

            // 2. Читаем список сохраненных сетей
            var res = await _session.Tcp.GetWifiNetworksAsync();
            Networks.Clear();
            if (res?.Networks != null)
            {
                foreach (var net in res.Networks)
                {
                    Networks.Add(net);
                }
                StatusMessage = $"Сохранено сетей: {Networks.Count}";
            }
        }
        catch (Exception ex)
        {
            StatusMessage = $"Ошибка чтения: {ex.Message}";
        }
        finally
        {
            IsBusy = false;
        }
    }

    [RelayCommand]
    public async Task AddNetworkAsync()
    {
        if (string.IsNullOrWhiteSpace(NewSsid))
        {
            StatusMessage = "Имя сети (SSID) не может быть пустым!";
            return;
        }

        IsBusy = true;
        StatusMessage = $"Сохранение сети «{NewSsid}»...";
        try
        {
            bool ok = await _session.Tcp.SetWifiNetworkAsync(NewSsid.Trim(), NewPassword.Trim(), NewPriority);
            if (ok)
            {
                NewSsid = "";
                NewPassword = "";
                await RefreshListAsync();
                StatusMessage = "Сеть сохранена в робота!";
            }
            else
            {
                StatusMessage = "Робот отклонил запись сети";
            }
        }
        catch (Exception ex)
        {
            StatusMessage = $"Ошибка записи: {ex.Message}";
        }
        finally
        {
            IsBusy = false;
        }
    }

    [RelayCommand]
    public async Task DeleteNetworkAsync(string ssid)
    {
        IsBusy = true;
        StatusMessage = $"Удаление сети «{ssid}»...";
        try
        {
            bool ok = await _session.Tcp.DeleteWifiNetworkAsync(ssid);
            if (ok)
            {
                await RefreshListAsync();
                StatusMessage = $"Сеть «{ssid}» удалена!";
            }
        }
        catch (Exception ex)
        {
            StatusMessage = $"Ошибка удаления: {ex.Message}";
        }
        finally
        {
            IsBusy = false;
        }
    }
}