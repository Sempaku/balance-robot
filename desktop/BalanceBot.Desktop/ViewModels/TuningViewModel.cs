using System.Collections.ObjectModel;
using BalanceBot.Core.Tuning;
using BalanceBot.Protocol.Dtos;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace BalanceBot.Desktop.ViewModels;

public sealed partial class TuningViewModel : ObservableObject
{
    private readonly ParameterManager _paramManager;

    public ObservableCollection<ParamItemViewModel> Parameters { get; } = new();

    [ObservableProperty] private string _statusMessage = "Готово к настройке";
    [ObservableProperty] private bool _isBusy;

    public TuningViewModel(ParameterManager paramManager)
    {
        _paramManager = paramManager;
        BuildParamList();
    }

    public void BuildParamList()
    {
        Parameters.Clear();
        foreach (var item in _paramManager.Catalog)
        {
            float cur = _paramManager.CurrentValues.GetValueOrDefault(item.Key, item.DefaultValue);
            var vm = new ParamItemViewModel(item, cur, val =>
            {
                _paramManager.SetLocal(item.Key, val);
            });
            Parameters.Add(vm);
        }
    }

    [RelayCommand]
    public async Task ApplyAsync()
    {
        IsBusy = true;
        StatusMessage = "Применение параметров в RAM...";
        try
        {
            bool ok = await _paramManager.ApplyModifiedAsync();
            StatusMessage = ok ? "Параметры успешно применены!" : "Ошибка применения параметров";
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

    [RelayCommand]
    public async Task SaveToNvsAsync()
    {
        IsBusy = true;
        StatusMessage = "Сохранение параметров в NVS память робота...";
        try
        {
            bool ok = await _paramManager.SaveToNvsAsync();
            StatusMessage = ok ? "Сохранено в NVS! Переживут перезагрузку." : "Ошибка записи NVS";
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

    [RelayCommand]
    public async Task ResetToDefaultsAsync()
    {
        IsBusy = true;
        StatusMessage = "Сброс к заводским настройкам...";
        try
        {
            bool ok = await _paramManager.ResetToDefaultsAsync();
            if (ok)
            {
                BuildParamList();
                StatusMessage = "Сброшено к зашитым по умолчанию!";
            }
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

/// <summary>
/// Модель элемента настройки для привязки к слайдеру.
/// Использует чистый SetProperty без конфликтов Source Generator.
/// </summary>
public sealed class ParamItemViewModel : ObservableObject
{
    private readonly Action<float> _onChanged;
    private float _currentValue;

    public ParamCatalogItem Meta { get; }

    public float CurrentValue
    {
        get => _currentValue;
        set
        {
            if (SetProperty(ref _currentValue, value))
            {
                _onChanged(value);
            }
        }
    }

    public ParamItemViewModel(ParamCatalogItem meta, float value, Action<float> onChanged)
    {
        Meta = meta;
        _currentValue = value;
        _onChanged = onChanged;
    }
}