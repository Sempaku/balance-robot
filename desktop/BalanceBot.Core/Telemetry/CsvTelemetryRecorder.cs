using System.Globalization;
using System.IO;
using System.Text;
using BalanceBot.Protocol.Packets;

namespace BalanceBot.Core.Telemetry;

/// <summary>
/// Регистратор сессий телеметрии в CSV-файл для анализа в Excel, Python, MATLAB.
/// </summary>
public sealed class CsvTelemetryRecorder : IAsyncDisposable
{
    private StreamWriter? _writer;
    private readonly object _lock = new();

    public bool IsRecording { get; private set; }
    public string? CurrentFilePath { get; private set; }
    public ulong RecordsWritten { get; private set; }

    /// <summary>
    /// Начинает запись в удобную папку "Документы/BalanceBot/Logs".
    /// </summary>
    public void Start(string? directoryPath = null)
    {
        lock (_lock)
        {
            if (IsRecording) return;

            // Сохраняем в папку "Мои Документы/BalanceBot/Logs" для легкого доступа
            directoryPath ??= Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "BalanceBot", "Logs");
            Directory.CreateDirectory(directoryPath);

            string fileName = $"telemetry_{DateTime.Now:yyyy-MM-dd_HH-mm-ss}.csv";
            CurrentFilePath = Path.Combine(directoryPath, fileName);

            var stream = new FileStream(CurrentFilePath, FileMode.Create, FileAccess.Write, FileShare.Read, bufferSize: 16384, useAsync: true);
            _writer = new StreamWriter(stream, Encoding.UTF8);

            // Заголовок CSV
            _writer.WriteLine("UptimeMs,Angle,AccelAngle,Gyro,AccelMag,GyroBias,Trim,Cmd,WheelRate,TermP,TermLean,TermD,Steps,DtUs,LoopUs,State,Flags,ImuErrors");
            _writer.Flush();

            RecordsWritten = 0;
            IsRecording = true;
        }
    }

    /// <summary>
    /// Запись одного кадра телеметрии.
    /// </summary>
    public void Write(in TelemetryPacket p)
    {
        if (!IsRecording || _writer == null) return;

        lock (_lock)
        {
            if (!IsRecording || _writer == null) return;

            var c = CultureInfo.InvariantCulture;

            _writer.WriteLine(
                $"{p.UptimeMs}," +
                $"{p.Angle.ToString("F3", c)}," +
                $"{p.AccelAngle.ToString("F3", c)}," +
                $"{p.Gyro.ToString("F2", c)}," +
                $"{p.AccelMag.ToString("F3", c)}," +
                $"{p.GyroBias.ToString("F3", c)}," +
                $"{p.Trim.ToString("F3", c)}," +
                $"{p.Cmd.ToString("F1", c)}," +
                $"{p.WheelRate.ToString("F1", c)}," +
                $"{p.TermP.ToString("F1", c)}," +
                $"{p.TermLean.ToString("F2", c)}," +
                $"{p.TermD.ToString("F1", c)}," +
                $"{p.Steps}," +
                $"{p.DtUs}," +
                $"{p.LoopUs}," +
                $"{(byte)p.State}," +
                $"{p.Flags}," +
                $"{p.ImuErrors}"
            );

            RecordsWritten++;

            // Каждые 50 строк (полсекунды) сбрасываем буфер на диск, чтобы файл не казался пустым
            if (RecordsWritten % 50 == 0)
            {
                _writer.Flush();
            }
        }
    }

    /// <summary>
    /// Завершение записи и закрытие файла.
    /// </summary>
    public async Task StopAsync()
    {
        StreamWriter? w;
        lock (_lock)
        {
            if (!IsRecording) return;
            IsRecording = false;
            w = _writer;
            _writer = null;
        }

        if (w != null)
        {
            await w.FlushAsync();
            await w.DisposeAsync();
        }
    }

    public async ValueTask DisposeAsync()
    {
        await StopAsync();
    }
}