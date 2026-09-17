namespace BalanceBot.Core.Analysis;

/// <summary>
/// Точка частотной характеристики: частота, модуль (дБ) и фаза (градусы).
/// </summary>
public readonly record struct FrequencyPoint(double FreqHz, double MagDb, double PhaseDeg);

/// <summary>
/// Результат частотного анализа системы балансировки.
/// </summary>
public sealed class BodeResult
{
    /// <summary>Замкнутая передаточная функция T: трим → угол (дБ / градусы).</summary>
    public required IReadOnlyList<FrequencyPoint> ClosedLoop { get; init; }

    /// <summary>Восстановленный контур L = T/(1−T) (дБ / градусы).</summary>
    public required IReadOnlyList<FrequencyPoint> Loop { get; init; }

    /// <summary>Фактическая частота дискретизации телеметрии (Гц).</summary>
    public required double SampleRateHz { get; init; }

    /// <summary>Число отсчётов, использованное в БПФ.</summary>
    public required int SampleCount { get; init; }

    /// <summary>Запас по усилению (дБ). null — если пересечение −180° не найдено.</summary>
    public double? GainMarginDb { get; init; }

    /// <summary>Запас по фазе (градусы). null — если пересечение 0 дБ не найдено.</summary>
    public double? PhaseMarginDeg { get; init; }

    /// <summary>Частота среза по усилению (|L| = 0 дБ), Гц.</summary>
    public double? GainCrossoverHz { get; init; }

    /// <summary>Частота среза по фазе (∠L = −180°), Гц.</summary>
    public double? PhaseCrossoverHz { get; init; }

    /// <summary>Пик АЧХ замкнутого контура |T| (дБ) — резонанс.</summary>
    public double PeakClosedLoopDb { get; init; }

    /// <summary>Частота пика |T| (Гц).</summary>
    public double PeakClosedLoopHz { get; init; }

    /// <summary>
    /// CSV для экспорта: частота, АЧХ/ФЧХ замкнутой системы и контура.
    /// </summary>
    public string ToCsv()
    {
        var sb = new System.Text.StringBuilder();
        sb.AppendLine("freq_hz,t_mag_db,t_phase_deg,l_mag_db,l_phase_deg");

        var c = System.Globalization.CultureInfo.InvariantCulture;
        for (int i = 0; i < Loop.Count; i++)
        {
            var l = Loop[i];
            var t = i < ClosedLoop.Count ? ClosedLoop[i] : new FrequencyPoint();
            sb.Append(l.FreqHz.ToString("F4", c)).Append(',');
            sb.Append(t.MagDb.ToString("F2", c)).Append(',');
            sb.Append(t.PhaseDeg.ToString("F2", c)).Append(',');
            sb.Append(l.MagDb.ToString("F2", c)).Append(',');
            sb.AppendLine(l.PhaseDeg.ToString("F2", c));
        }

        return sb.ToString();
    }
}

/// <summary>
/// Анализатор устойчивости: эмпирическая оценка передаточной функции через БПФ
/// (ETFE — Empirical Transfer Function Estimate) и расчёт запасов устойчивости.
///
/// Методика: на робота подаётся импульс трима (известное опорное воздействие r),
/// записывается отклик угла y. Оценивается замкнутая ПФ T(jw) = Y/R,
/// после чего восстанавливается контур L(jw) = T/(1−T) и по нему классически
/// вычисляются запасы по усилению и фазе.
/// </summary>
public static class BodeAnalyzer
{
    /// <summary>Нижняя граница анализируемого диапазона частот (Гц).</summary>
    public const double MinFreqHz = 0.3;

    /// <summary>Верхняя граница анализируемого диапазона частот (Гц) — выше телеметрия зашумлена.</summary>
    public const double MaxFreqHz = 25.0;

    /// <summary>Относительный порог энергии входа: бины слабее −40 дБ отбрасываются.</summary>
    private const double InputPowerThreshold = 1e-4;

    /// <summary>Бины, где |1−T| меньше этого значения, дают неустойчивую оценку L — пропускаются.</summary>
    private const double MinLoopDenominator = 0.05;

    /// <summary>
    /// Оценка частоты дискретизации по меткам времени телеметрии (медиана интервалов).
    /// </summary>
    public static double EstimateSampleRateHz(IReadOnlyList<uint> uptimeMs, double fallbackHz = 100.0)
    {
        if (uptimeMs.Count < 4)
        {
            return fallbackHz;
        }

        var deltas = new List<int>(uptimeMs.Count - 1);
        for (int i = 1; i < uptimeMs.Count; i++)
        {
            int d = (int)(uptimeMs[i] - uptimeMs[i - 1]);
            if (d is > 0 and < 200)
            {
                deltas.Add(d);
            }
        }

        if (deltas.Count == 0)
        {
            return fallbackHz;
        }

        deltas.Sort();
        double medianMs = deltas[deltas.Count / 2];
        return medianMs > 0 ? 1000.0 / medianMs : fallbackHz;
    }

    /// <summary>
    /// Строит частотные характеристики по записанным сигналам.
    /// input — опорное воздействие (трим, °), output — отклик (угол, °).
    /// Возвращает null, если данных недостаточно.
    /// </summary>
    public static BodeResult? Analyze(double[] input, double[] output, double sampleRateHz)
    {
        int count = Math.Min(input.Length, output.Length);
        int n = Fft.TruncateToPowerOfTwo(count);

        // Нужно хотя бы 256 отсчётов для разумного разрешения по частоте
        if (n < 256 || sampleRateHz <= 1.0)
        {
            return null;
        }

        var x = new double[n];
        var y = new double[n];
        Array.Copy(input, x, n);
        Array.Copy(output, y, n);

        // Удаление постоянной составляющей (дрейф нуля угла и трима)
        RemoveMean(x);
        RemoveMean(y);

        // Окно Ханна — снижает утечку спектра из-за конечности записи
        ApplyHann(x);
        ApplyHann(y);

        var (ux, uy) = Fft.Forward(x);
        var (vx, vy) = Fft.Forward(y);

        int half = n / 2;
        double maxInputPower = 0;
        for (int k = 1; k <= half; k++)
        {
            double p = ux[k] * ux[k] + uy[k] * uy[k];
            if (p > maxInputPower) maxInputPower = p;
        }

        if (maxInputPower <= 0)
        {
            return null;
        }

        double threshold = maxInputPower * InputPowerThreshold;
        double binHz = sampleRateHz / n;

        // ETFE: T(jw) = Y(jw)/U(jw) на бинах с достаточной энергией входа
        var tPoints = new List<FrequencyPoint>(half);
        var tComplex = new List<(double Re, double Im)>(half);
        for (int k = 1; k <= half; k++)
        {
            double ur = ux[k], ui = uy[k];
            double p = ur * ur + ui * ui;
            if (p <= threshold) continue;

            double yr = vx[k], yi = vy[k];
            // T = Y / U = Y * conj(U) / |U|^2
            double tr = (yr * ur + yi * ui) / p;
            double ti = (yi * ur - yr * ui) / p;

            double freq = k * binHz;
            if (freq < MinFreqHz || freq > Math.Min(MaxFreqHz, sampleRateHz / 2.0)) continue;

            tComplex.Add((tr, ti));
            tPoints.Add(new FrequencyPoint(freq, MagDb(tr, ti), Math.Atan2(ti, tr) * RadToDeg));
        }

        if (tPoints.Count < 8)
        {
            return null;
        }

        UnwrapPhases(tPoints);

        // Контур: L = T / (1 − T)
        var loop = new List<FrequencyPoint>(tPoints.Count);
        for (int i = 0; i < tComplex.Count; i++)
        {
            var t = tComplex[i];
            double dr = 1.0 - t.Re;
            double di = -t.Im;
            double den = Math.Sqrt(dr * dr + di * di);
            // T ≈ 1 (на границе устойчивости) — оценка L ненадёжна, бин пропускаем
            if (den < MinLoopDenominator) continue;

            double den2 = den * den;
            double lr = (t.Re * dr + t.Im * di) / den2;
            double li = (t.Im * dr - t.Re * di) / den2;

            loop.Add(new FrequencyPoint(tPoints[i].FreqHz, MagDb(lr, li), Math.Atan2(li, lr) * RadToDeg));
        }

        if (loop.Count < 8)
        {
            return null;
        }

        UnwrapPhases(loop);

        var (gmDb, pmDeg, fcGain, fcPhase) = ComputeMargins(loop);

        // Пик замкнутой АЧХ (резонанс)
        double peakDb = double.MinValue;
        double peakHz = 0;
        foreach (var p in tPoints)
        {
            if (p.MagDb > peakDb)
            {
                peakDb = p.MagDb;
                peakHz = p.FreqHz;
            }
        }

        return new BodeResult
        {
            ClosedLoop = tPoints,
            Loop = loop,
            SampleRateHz = sampleRateHz,
            SampleCount = n,
            GainMarginDb = gmDb,
            PhaseMarginDeg = pmDeg,
            GainCrossoverHz = fcGain,
            PhaseCrossoverHz = fcPhase,
            PeakClosedLoopDb = peakDb,
            PeakClosedLoopHz = peakHz,
        };
    }

    /// <summary>
    /// Классические запасы устойчивости по контуру L:
    /// запас по фазе — на пересечении |L| с 0 дБ,
    /// запас по усилению — на пересечении фазы с −180° (нечётными кратными 180°).
    /// Возвращается худшее (минимальное) значение среди всех пересечений.
    /// </summary>
    private static (double? GmDb, double? PmDeg, double? FcGain, double? FcPhase) ComputeMargins(
        IReadOnlyList<FrequencyPoint> loop)
    {
        double? bestGm = null, gmFreq = null;
        double? bestPm = null, pmFreq = null;

        for (int i = 1; i < loop.Count; i++)
        {
            var a = loop[i - 1];
            var b = loop[i];

            // Пересечение уровня 0 дБ → запас по фазе
            double la = DbToLin(a.MagDb);
            double lb = DbToLin(b.MagDb);
            if ((la - 1.0) * (lb - 1.0) < 0)
            {
                double t = (1.0 - la) / (lb - la);
                double f = a.FreqHz + t * (b.FreqHz - a.FreqHz);
                double ph = a.PhaseDeg + t * (b.PhaseDeg - a.PhaseDeg);
                double pm = 180.0 + WrapPhase(ph);

                if (bestPm == null || pm < bestPm)
                {
                    bestPm = pm;
                    pmFreq = f;
                }
            }

            // Пересечение фазы с −180° + k·360° → запас по усилению
            double ka = Math.Floor((a.PhaseDeg + 180.0) / 360.0);
            double kb = Math.Floor((b.PhaseDeg + 180.0) / 360.0);
            if (ka != kb)
            {
                // Пересечённый уровень — граница между диапазонами ka и kb
                double target = Math.Max(ka, kb) * 360.0 - 180.0;
                double t = Math.Clamp((target - a.PhaseDeg) / (b.PhaseDeg - a.PhaseDeg), 0.0, 1.0);
                double f = a.FreqHz + t * (b.FreqHz - a.FreqHz);
                double magDb = a.MagDb + t * (b.MagDb - a.MagDb);
                double gm = -magDb;

                if (bestGm == null || gm < bestGm)
                {
                    bestGm = gm;
                    gmFreq = f;
                }
            }
        }

        return (bestGm, bestPm, pmFreq, gmFreq);
    }

    private static void RemoveMean(double[] s)
    {
        double sum = 0;
        foreach (double v in s) sum += v;
        double mean = sum / s.Length;
        for (int i = 0; i < s.Length; i++) s[i] -= mean;
    }

    private static void ApplyHann(double[] s)
    {
        int n = s.Length;
        for (int i = 0; i < n; i++)
        {
            s[i] *= 0.5 - 0.5 * Math.Cos(2.0 * Math.PI * i / n);
        }
    }

    /// <summary>Разворачивает фазу (устраняет скачки на ±360°).</summary>
    private static void UnwrapPhases(List<FrequencyPoint> points)
    {
        for (int i = 1; i < points.Count; i++)
        {
            double prev = points[i - 1].PhaseDeg;
            double cur = points[i].PhaseDeg;
            double d = cur - prev;
            while (d > 180.0) { cur -= 360.0; d = cur - prev; }
            while (d < -180.0) { cur += 360.0; d = cur - prev; }
            points[i] = points[i] with { PhaseDeg = cur };
        }
    }

    private static double WrapPhase(double deg)
    {
        double w = ((deg + 180.0) % 360.0 + 360.0) % 360.0 - 180.0;
        return w;
    }

    private static double MagDb(double re, double im) => 20.0 * Math.Log10(Math.Sqrt(re * re + im * im) + 1e-12);

    private static double DbToLin(double db) => Math.Pow(10.0, db / 20.0);

    private const double RadToDeg = 180.0 / Math.PI;
}
