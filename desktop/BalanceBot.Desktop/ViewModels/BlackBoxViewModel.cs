using BalanceBot.Core;
using BalanceBot.Protocol.Dtos;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using ScottPlot;

namespace BalanceBot.Desktop.ViewModels;

public sealed partial class BlackBoxViewModel : ObservableObject
{
    private readonly RobotSession _session;

    [ObservableProperty] private string _statusMessage = "Нажмите «Выгрузить», чтобы получить кадры аварии";
    [ObservableProperty] private bool _isBusy;
    [ObservableProperty] private int _frameCount;

    public Action<List<BlackBoxFrameDto>>? OnFramesLoaded { get; set; }

    public BlackBoxViewModel(RobotSession session)
    {
        _session = session;
    }

    [RelayCommand]
    public async Task LoadBlackBoxAsync()
    {
        IsBusy = true;
        StatusMessage = "Запрос черного ящика по TCP...";
        try
        {
            var res = await _session.Tcp.GetBlackBoxAsync();
            if (res?.Ok == true && res.Frames != null && res.Frames.Count > 0)
            {
                FrameCount = res.Frames.Count;
                StatusMessage = $"Загружено {FrameCount} кадров аварии (полная частота 200 Гц)!";
                OnFramesLoaded?.Invoke(res.Frames);
            }
            else
            {
                StatusMessage = "Черный ящик пуст. Робот еще не падал после арминга.";
            }
        }
        catch (Exception ex)
        {
            StatusMessage = $"Ошибка выгрузки: {ex.Message}";
        }
        finally
        {
            IsBusy = false;
        }
    }

    [RelayCommand]
    public async Task ClearBlackBoxAsync()
    {
        IsBusy = true;
        try
        {
            bool ok = await _session.Tcp.ClearBlackBoxAsync();
            StatusMessage = ok ? "Память чёрного ящика очищена!" : "Ошибка очистки";
            FrameCount = 0;
        }
        catch (Exception ex)
        {
            StatusMessage = $"Ошибка: {ex.Message}";
        }
        finally
        {
            IsBusy = false;
        }
    }
}