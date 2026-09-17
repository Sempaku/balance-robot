namespace BalanceBot.Core.Analysis;

/// <summary>
/// Итеративное БПФ по основанию 2 (Кули-Тьюки) с перестановкой битреверсом.
/// Внешних зависимостей нет — достаточно System.Math.
/// </summary>
internal static class Fft
{
    /// <summary>
    /// Прямое преобразование Фурье вещественного сигнала.
    /// Длина массива должна быть степенью двойки.
    /// </summary>
    public static (double[] Re, double[] Im) Forward(double[] samples)
    {
        int n = samples.Length;
        if (!IsPowerOfTwo(n))
        {
            throw new ArgumentException($"Длина сигнала должна быть степенью двойки, получено {n}");
        }

        var re = (double[])samples.Clone();
        var im = new double[n];

        // Битреверс-перестановка индексов
        for (int i = 1, j = 0; i < n; i++)
        {
            int bit = n >> 1;
            for (; (j & bit) != 0; bit >>= 1)
            {
                j ^= bit;
            }
            j ^= bit;

            if (i < j)
            {
                (re[i], re[j]) = (re[j], re[i]);
                (im[i], im[j]) = (im[j], im[i]);
            }
        }

        // Бабочки
        for (int len = 2; len <= n; len <<= 1)
        {
            double ang = -2.0 * Math.PI / len;
            double wr = Math.Cos(ang);
            double wi = Math.Sin(ang);

            for (int i = 0; i < n; i += len)
            {
                double cr = 1.0;
                double ci = 0.0;

                for (int k = 0; k < len / 2; k++)
                {
                    int a = i + k;
                    int b = i + k + len / 2;

                    double tr = re[b] * cr - im[b] * ci;
                    double ti = re[b] * ci + im[b] * cr;

                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;

                    (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
                }
            }
        }

        return (re, im);
    }

    public static bool IsPowerOfTwo(int value) => value > 0 && (value & (value - 1)) == 0;

    /// <summary>
    /// Наибольшая степень двойки, не превышающая value (минимум 1).
    /// </summary>
    public static int TruncateToPowerOfTwo(int value)
    {
        int p = 1;
        while (p * 2 <= value)
        {
            p <<= 1;
        }
        return p;
    }
}
