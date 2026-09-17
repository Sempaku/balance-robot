using BalanceBot.Protocol;
using BalanceBot.Protocol.Packets;
using BalanceBot.Core.Telemetry;
using BalanceBot.Core.Tuning;

namespace BalanceBot.Core.Analysis;

/// <summary>
/// Результат импульсного теста: записанные сигналы и частота дискретизации.
/// </summary>
public sealed record PulseTestResult(
    double[] TimeS,
    double[] Trim,
    double[] Angle,
    double SampleRateHz);

/// <summary>
/// Импульсный тест контура балансировки: робот стоит на полу в режиме БАЛАНС,
/// приложение подаёт короткий импульс трима и записывает отклик угла.
/// Используется вкладкой Bode и автонастройкой PID.
/// </summary>
public sealed class PulseTestRunner
{
    // Тайминги теста (мс): базовая линия → импульс трима → возврат → успокоение.
    // Сумма 6.5 с при телеметрии 100 Гц даёт ~650 кадров → БПФ на 512 точках.
    private const int BaselineMs = 1000;
    private const int PulseMs = 1500;
    private const int SettleMs = 4000;

    private const float TrimLimitDeg = 8.0f;
    private const int MaxFrames = 8192;

    private readonly TelemetryHub _telemetry;
    private readonly ParameterManager _parameters;

    public PulseTestRunner(TelemetryHub telemetry, ParameterManager parameters)
    {
        _telemetry = telemetry;
        _parameters = parameters;
    }

    /// <summary>
    /// Проверяет, что робот сейчас в режиме балансировки.
    /// </summary>
    public bool IsBalancing => (_telemetry.Latest.Flags & ProtocolConstants.FlagArmed) != 0;

    /// <summary>
    /// Запускает импульс трима и записывает отклик. Требует включённый баланс.
    /// Возвращает null, если робот не в балансе, упал за время теста или данных мало.
    /// </summary>
    public async Task<PulseTestResult?> RunAsync(float amplitudeDeg, CancellationToken ct = default)
    {
        if (!IsBalancing)
        {
            return null;
        }

        if (!_parameters.CurrentValues.TryGetValue("trim_offset", out float baseTrim))
        {
            baseTrim = 0f;
        }

        float pulseTarget = Math.Clamp(baseTrim + amplitudeDeg, -TrimLimitDeg, TrimLimitDeg);

        var capture = new List<TelemetryPacket>(1024);
        var gate = new object();

        void OnPacket(TelemetryPacket p)
        {
            lock (gate)
            {
                if (capture.Count < MaxFrames) capture.Add(p);
            }
        }

        _telemetry.OnPacketReceived += OnPacket;

        try
        {
            await Task.Delay(BaselineMs, ct);
            await SetTrimAsync(pulseTarget);
            await Task.Delay(PulseMs, ct);
            await SetTrimAsync(baseTrim);
            await Task.Delay(SettleMs, ct);
        }
        finally
        {
            _telemetry.OnPacketReceived -= OnPacket;

            // Гарантированный возврат исходного трима
            try
            {
                await SetTrimAsync(baseTrim);
            }
            catch
            {
                // связь могла оборваться — трим вернётся при следующем применении параметров
            }
        }

        // Робот упал или был выключен во время теста — данные непригодны
        if (!IsBalancing || capture.Count < 256)
        {
            return null;
        }

        double fs = BodeAnalyzer.EstimateSampleRateHz(capture.Select(f => f.UptimeMs).ToArray());

        double[] time = new double[capture.Count];
        double[] trim = new double[capture.Count];
        double[] angle = new double[capture.Count];
        uint t0 = capture[0].UptimeMs;
        for (int i = 0; i < capture.Count; i++)
        {
            time[i] = (capture[i].UptimeMs - t0) / 1000.0;
            trim[i] = capture[i].Trim;
            angle[i] = capture[i].Angle;
        }

        return new PulseTestResult(time, trim, angle, fs);
    }

    private async Task SetTrimAsync(float value)
    {
        _parameters.SetLocal("trim_offset", value);
        await _parameters.ApplyModifiedAsync();
    }
}
