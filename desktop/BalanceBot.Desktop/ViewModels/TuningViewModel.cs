using System.Collections.ObjectModel;
using BalanceBot.Core;
using BalanceBot.Core.Analysis;
using BalanceBot.Core.Tuning;
using BalanceBot.Protocol.Dtos;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace BalanceBot.Desktop.ViewModels;

public sealed partial class TuningViewModel : ObservableObject
{
    private readonly RobotSession _session;
    private readonly ParameterManager _paramManager;
    private readonly PulseTestRunner _testRunner;

    // Итеративная настройка: измерение (6.5 с) + успокоение после смены коэффициентов
    private static readonly TimeSpan SettleAfterAdjust = TimeSpan.FromMilliseconds(2500);
    private const int MaxIterations = 4;

    // Автоприменение изменений на лету: после отпускания слайдера ждём 400 мс
    // и применяем скопом (одна TCP-транзакция вместо потока на каждый пиксель)
    private const int AutoApplyDelayMs = 400;

    // Целевой запас по фазе: ниже 40° — звенит, выше 65° — вялое управление
    private const double TargetPmMin = 40.0;
    private const double TargetPmMax = 65.0;

    // Диапазоны параметров прошивки (params.rs)
    private const float KaMin = 100.0f, KaMax = 12000.0f;
    private const float KdMin = 0.0f, KdMax = 3000.0f;

    public ObservableCollection<ParamItemViewModel> Parameters { get; } = new();

    private CancellationTokenSource? _autoApplyCts;

    [ObservableProperty] private string _statusMessage = "Готово к настройке: двигайте слайдеры — изменения уходят на робота на лету";
    [ObservableProperty] private bool _isBusy;

    [ObservableProperty] private bool _isAutotuneRunning;
    [ObservableProperty] private string _autotuneStatusText =
        "Поставьте робота на пол и включите БАЛАНС, затем нажмите кнопку: " +
        "приложение измерит частотную характеристику и само подберёт Ka/Kd";
    [ObservableProperty] private string _autotuneResultText = "";
    [ObservableProperty] private bool _hasAutotuneResult;

    private Dictionary<string, float>? _preAutotuneValues;

    public TuningViewModel(RobotSession session)
    {
        _session = session;
        _paramManager = session.Parameters;
        _testRunner = new PulseTestRunner(session.Telemetry, session.Parameters);
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
                ScheduleAutoApply();
            });
            Parameters.Add(vm);
        }
    }

    /// <summary>
    /// Автоприменение на лету: слайдер отпущен — через 400 мс изменения уходят
    /// на робота одним пакетом. Кнопка «Применить (RAM)» делает то же немедленно.
    /// </summary>
    private void ScheduleAutoApply()
    {
        _autoApplyCts?.Cancel();
        _autoApplyCts = new CancellationTokenSource();
        var token = _autoApplyCts.Token;
        _ = AutoApplyAsync(token);
    }

    private async Task AutoApplyAsync(CancellationToken token)
    {
        try
        {
            await Task.Delay(AutoApplyDelayMs, token);
        }
        catch (TaskCanceledException)
        {
            return;
        }

        if (token.IsCancellationRequested || _paramManager.ModifiedValues.Count == 0)
        {
            return;
        }

        try
        {
            bool ok = await _paramManager.ApplyModifiedAsync();
            StatusMessage = ok
                ? "Изменения применены на лету (RAM). Для фиксации нажмите «Сохранить в NVS»."
                : "Робот не принял изменения — проверьте связь и повторите.";
        }
        catch (Exception ex)
        {
            StatusMessage = $"Ошибка автоприменения: {ex.Message}";
        }
    }

    private void CancelAutoApply()
    {
        _autoApplyCts?.Cancel();
        _autoApplyCts = null;
    }

    [RelayCommand]
    public async Task ApplyAsync()
    {
        CancelAutoApply();
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
        CancelAutoApply();
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
        CancelAutoApply();
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

    /// <summary>
    /// Автонастройка PID на полу: робот стоит в обычном режиме БАЛАНС,
    /// приложение прогоняет импульсные тесты, измеряет запас по фазе
    /// контура и итеративно подводит Ka/Kd к целевому диапазону 40-65°.
    /// </summary>
    [RelayCommand]
    public async Task RunAutotuneAsync()
    {
        if (IsAutotuneRunning) return;

        if (!_testRunner.IsBalancing)
        {
            AutotuneStatusText = "Поставьте робота на пол и ВКЛЮЧИТЕ БАЛАНС — автонастройка идёт в обычном режиме, руки не нужны.";
            return;
        }

        IsAutotuneRunning = true;
        HasAutotuneResult = false;
        AutotuneResultText = "";
        AutotuneStatusText = "Автонастройка идёт: робот стоит на полу, не мешайте ему (~10-40 с)...";

        _preAutotuneValues = new Dictionary<string, float>();
        foreach (var key in new[] { "ka", "kd", "ki" })
        {
            if (_paramManager.CurrentValues.TryGetValue(key, out float v))
            {
                _preAutotuneValues[key] = v;
            }
        }

        float oldKa = _preAutotuneValues.GetValueOrDefault("ka");
        float oldKd = _preAutotuneValues.GetValueOrDefault("kd");
        float oldKi = _preAutotuneValues.GetValueOrDefault("ki");

        try
        {
            var result = await MeasureAsync("измерение исходной АЧХ");
            if (result == null)
            {
                AutotuneStatusText = "Автонастройка не удалась: не удалось измерить отклик (робот упал? связь?).";
                return;
            }

            double? pmFirst = result.PhaseMarginDeg;

            for (int iter = 1; iter <= MaxIterations; iter++)
            {
                double? pm = result.PhaseMarginDeg;

                // Уже в целевом диапазоне — готово
                if (pm is >= TargetPmMin and <= TargetPmMax)
                {
                    break;
                }

                float ka = _paramManager.CurrentValues.GetValueOrDefault("ka");
                float kd = _paramManager.CurrentValues.GetValueOrDefault("kd");

                double kaFactor = 1.0, kdFactor = 1.0;
                string reason;

                if (pm == null)
                {
                    // Контур не пересекает 0 дБ: |L| везде ниже — усиления мало,
                    // везде выше — контур на грани самовозбуждения
                    double maxLoopDb = result.Loop.Max(p => p.MagDb);
                    if (maxLoopDb < 0)
                    {
                        kaFactor = 1.6;
                        reason = "усиление мало (АЧХ не дотягивает до 0 дБ)";
                    }
                    else
                    {
                        kaFactor = 0.6;
                        reason = "контур всё время выше 0 дБ — опасно, снижаем Ka";
                    }
                }
                else if (pm < TargetPmMin)
                {
                    kaFactor = 0.85;
                    kdFactor = result.PeakClosedLoopDb > 6.0 ? 1.25 : 1.15;
                    reason = $"запас по фазе мал ({pm:F0}° < {TargetPmMin:F0}°)";
                }
                else
                {
                    kaFactor = 1.3;
                    reason = $"запас по фазе велик ({pm:F0}° > {TargetPmMax:F0}°), управление вялое";
                }

                float newKa = Math.Clamp(ka * (float)kaFactor, KaMin, KaMax);
                float newKd = Math.Clamp(kd * (float)kdFactor, KdMin, KdMax);

                if (Math.Abs(newKa - ka) < 0.5f && Math.Abs(newKd - kd) < 0.5f)
                {
                    break; // упёрлись в границы диапазона
                }

                AutotuneStatusText = $"Итерация {iter}/{MaxIterations}: {reason} → Ka {ka:F0}→{newKa:F0}, Kd {kd:F0}→{newKd:F0}";

                _paramManager.SetLocal("ka", newKa);
                _paramManager.SetLocal("kd", newKd);
                bool applied = await _paramManager.ApplyModifiedAsync();
                if (!applied)
                {
                    AutotuneStatusText = "Автонастройка прервана: робот не принял новые коэффициенты.";
                    return;
                }

                await Task.Delay(SettleAfterAdjust);

                result = await MeasureAsync($"итерация {iter}: проверка");
                if (result == null)
                {
                    AutotuneStatusText = "Автонастройка прервана: робот упал или потеряна связь после смены коэффициентов. Откатите значения.";
                    return;
                }
            }

            float newKaVal = _paramManager.CurrentValues.GetValueOrDefault("ka");
            float newKdVal = _paramManager.CurrentValues.GetValueOrDefault("kd");
            float newKiVal = _paramManager.CurrentValues.GetValueOrDefault("ki");

            BuildParamList();

            double? pmLast = result.PhaseMarginDeg;
            string pmReport = pmFirst.HasValue && pmLast.HasValue
                ? $"запас по фазе {pmFirst:F0}° → {pmLast:F0}°"
                : "запас по фазе: не измерен";

            AutotuneStatusText = "Автонастройка завершена! Проверьте робота: погоняйте, наклоните. " +
                "Стабильно → «Сохранить в NVS», иначе → « Откатить».";
            AutotuneResultText =
                $"Ka: {oldKa:F0} → {newKaVal:F0}   Kd: {oldKd:F0} → {newKdVal:F0}   Ki: {oldKi:F1} → {newKiVal:F1}\n" +
                $"{pmReport}" +
                (result.GainCrossoverHz.HasValue ? $", частота среза {result.GainCrossoverHz:F2} Гц" : "");
            HasAutotuneResult = true;
            StatusMessage = "Автонастройка: новые коэффициенты применены в RAM. Проверьте робота и сохраните в NVS.";
        }
        catch (Exception ex)
        {
            AutotuneStatusText = $"Ошибка автонастройки: {ex.Message}. Коэффициенты можно откатить.";
        }
        finally
        {
            IsAutotuneRunning = false;
        }
    }

    /// <summary>
    /// Откат коэффициентов Ka/Kd/Ki на значения до автонастройки.
    /// </summary>
    [RelayCommand]
    public async Task RollbackAutotuneAsync()
    {
        if (_preAutotuneValues == null || _preAutotuneValues.Count == 0) return;

        CancelAutoApply();
        IsBusy = true;
        try
        {
            foreach (var (key, value) in _preAutotuneValues)
            {
                _paramManager.SetLocal(key, value);
            }

            bool ok = await _paramManager.ApplyModifiedAsync();
            if (ok)
            {
                BuildParamList();
                AutotuneStatusText = "Коэффициенты возвращены к значениям до автонастройки.";
                HasAutotuneResult = false;
            }
        }
        catch (Exception ex)
        {
            StatusMessage = $"Ошибка отката: {ex.Message}";
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task<BodeResult?> MeasureAsync(string stage)
    {
        var capture = await _testRunner.RunAsync(1.0f);
        if (capture == null)
        {
            return null;
        }

        return await Task.Run(() => BodeAnalyzer.Analyze(capture.Trim, capture.Angle, capture.SampleRateHz));
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

    /// <summary>Шаг стрелками клавиатуры: 1% диапазона.</summary>
    public double SmallChange => Math.Max((Meta.Max - Meta.Min) / 100.0, 1e-6);

    /// <summary>Шаг кликом по треку: 10% диапазона.</summary>
    public double LargeChange => Math.Max((Meta.Max - Meta.Min) / 10.0, 1e-5);

    /// <summary>Знаков после запятой для всплывающей подсказки слайдера.</summary>
    public int AutoToolTipPrecision
    {
        get
        {
            double range = Meta.Max - Meta.Min;
            if (range < 0.01) return 6;
            if (range < 0.1) return 5;
            if (range < 1.0) return 4;
            if (range < 10.0) return 2;
            if (range < 100.0) return 1;
            return 0;
        }
    }

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
