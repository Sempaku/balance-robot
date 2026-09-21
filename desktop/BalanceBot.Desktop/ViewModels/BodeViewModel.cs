using System.IO;
using BalanceBot.Core;
using BalanceBot.Core.Analysis;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.Win32;

namespace BalanceBot.Desktop.ViewModels;

/// <summary>
/// Вкладка анализа устойчивости: тест «импульс трима» → запись отклика угла →
/// построение диаграмм Боде и расчёт запасов по усилению и фазе.
/// </summary>
public sealed partial class BodeViewModel : ObservableObject
{
    private readonly PulseTestRunner _testRunner;

    /// <summary>Вызывается после теста с записанным откликом (время с, трим °, угол °).</summary>
    public Action<double[], double[], double[]>? OnStepResponse { get; set; }

    /// <summary>Вызывается после успешного частотного анализа.</summary>
    public Action<BodeResult>? OnAnalysisComplete { get; set; }

    [ObservableProperty] private bool _isRunning;

    [ObservableProperty] private float _pulseAmplitude = 1.0f;

    [ObservableProperty] private string _statusMessage =
        "Робот должен стоять на полу в режиме БАЛАНС. Нажмите «Запустить тест» — робот получит короткий импульс трима.";

    [ObservableProperty] private string _gainMarginText = "—";
    [ObservableProperty] private string _phaseMarginText = "—";
    [ObservableProperty] private string _crossoverText = "—";
    [ObservableProperty] private string _peakText = "—";
    [ObservableProperty] private bool _hasResult;

    private BodeResult? _lastResult;
    private CancellationTokenSource? _testCts;

    public BodeViewModel(RobotSession session)
    {
        _testRunner = new PulseTestRunner(session.Telemetry, session.Parameters);
    }

    [RelayCommand]
    public async Task RunTestAsync()
    {
        if (IsRunning) return;

        if (!_testRunner.IsBalancing)
        {
            StatusMessage = "Тест невозможен: поставьте робота на пол и включите режим БАЛАНС.";
            return;
        }

        IsRunning = true;
        HasResult = false;
        StatusMessage = "Тест идёт: не трогайте робот и трим...";

        _testCts?.Dispose();
        _testCts = new CancellationTokenSource();

        try
        {
            var capture = await _testRunner.RunAsync(PulseAmplitude, _testCts.Token);

            if (capture == null)
            {
                StatusMessage = "Тест не удался: робот не в балансе, упал за время теста или слабая телеметрия.";
                return;
            }

            OnStepResponse?.Invoke(capture.TimeS, capture.Trim, capture.Angle);

            var result = await Task.Run(() => BodeAnalyzer.Analyze(capture.Trim, capture.Angle, capture.SampleRateHz));

            if (result == null)
            {
                StatusMessage = "Анализ не удался: слишком слабый или зашумлённый сигнал трима.";
                return;
            }

            _lastResult = result;
            HasResult = true;

            GainMarginText = result.GainMarginDb.HasValue
                ? $"{result.GainMarginDb.Value:F1} дБ"
                : "не найден";
            PhaseMarginText = result.PhaseMarginDeg.HasValue
                ? $"{result.PhaseMarginDeg.Value:F1}°"
                : "не найден";
            CrossoverText = result.GainCrossoverHz.HasValue
                ? $"fср = {result.GainCrossoverHz.Value:F2} Гц"
                : "—";
            PeakText = $"{result.PeakClosedLoopDb:F1} дБ @ {result.PeakClosedLoopHz:F2} Гц";

            string verdict = (result.PhaseMarginDeg, result.GainMarginDb) switch
            {
                (< 30.0, _) or (_, < 6.0) => "Запасы малы — система близка к колебаниям!",
                (>= 30.0 and < 45.0, _) => "Запасы умеренные — допустимо, но следите за резонансом.",
                (>= 45.0 and <= 65.0, _) => "Запасы хорошие — система устойчива.",
                (_, _) => "Запасы очень большие — управление вялое, можно поднять Ka."
            };

            StatusMessage = $"Готово: {result.SampleCount} отсчётов, {capture.SampleRateHz:F0} Гц. {verdict}";
            OnAnalysisComplete?.Invoke(result);
        }
        catch (OperationCanceledException)
        {
            StatusMessage = "Тест прерван.";
        }
        catch (Exception ex)
        {
            StatusMessage = $"Ошибка теста: {ex.Message}";
        }
        finally
        {
            _testCts?.Dispose();
            _testCts = null;
            IsRunning = false;
        }
    }

    /// <summary>
    /// Прерывание текущего теста: отменяет ожидание фаз и запись отклика.
    /// </summary>
    [RelayCommand]
    public void StopTest()
    {
        _testCts?.Cancel();
    }

    [RelayCommand]
    public void ExportCsv()
    {
        if (_lastResult == null) return;

        var dialog = new SaveFileDialog
        {
            Filter = "CSV файлы (*.csv)|*.csv|Все файлы (*.*)|*.*",
            FileName = $"bode_{DateTime.Now:yyyy-MM-dd_HH-mm-ss}.csv",
        };

        if (dialog.ShowDialog() == true)
        {
            File.WriteAllText(dialog.FileName, _lastResult.ToCsv());
            StatusMessage = $"Частотная характеристика сохранена: {dialog.FileName}";
        }
    }
}
