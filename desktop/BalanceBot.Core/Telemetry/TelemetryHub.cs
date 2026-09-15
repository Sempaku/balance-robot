using System.Threading.Channels;
using BalanceBot.Protocol.Packets;

namespace BalanceBot.Core.Telemetry;

/// <summary>
/// Хаб телеметрии: принимает входящий поток пакетов из UDP,
/// наполняет кольцевой буфер истории и передает данные регистратору.
/// </summary>
public sealed class TelemetryHub : IAsyncDisposable
{
    private readonly ChannelReader<TelemetryPacket> _inputReader;
    private readonly CancellationTokenSource _cts = new();
    private Task? _processingTask;

    /// <summary>
    /// Кольцевой буфер истории (по умолчанию 6000 отсчетов = 60 секунд при 100 Гц).
    /// </summary>
    public RingBuffer<TelemetryPacket> History { get; }

    /// <summary>
    /// CSV-регистратор сессий на диск.
    /// </summary>
    public CsvTelemetryRecorder Recorder { get; } = new();

    /// <summary>
    /// Самый последний полученный срез телеметрии (для моментального обновления индикаторов в UI).
    /// </summary>
    public TelemetryPacket Latest { get; private set; }

    /// <summary>
    /// Событие обновления телеметрии (для подписки UI-таймера или других модулей).
    /// </summary>
    public event Action<TelemetryPacket>? OnPacketReceived;

    public TelemetryHub(ChannelReader<TelemetryPacket> inputReader, int historyCapacity = 6000)
    {
        _inputReader = inputReader;
        History = new RingBuffer<TelemetryPacket>(historyCapacity);
    }

    public void Start()
    {
        _processingTask = Task.Run(ProcessStreamAsync);
    }

    private async Task ProcessStreamAsync()
    {
        try
        {
            while (await _inputReader.WaitToReadAsync(_cts.Token))
            {
                while (_inputReader.TryRead(out var packet))
                {
                    Latest = packet;
                    History.Push(in packet);

                    if (Recorder.IsRecording)
                    {
                        Recorder.Write(in packet);
                    }

                    OnPacketReceived?.Invoke(packet);
                }
            }
        }
        catch (OperationCanceledException) { }
    }

    public async ValueTask DisposeAsync()
    {
        _cts.Cancel();
        if (_processingTask != null)
        {
            await Task.WhenAny(_processingTask, Task.Delay(500));
        }
        await Recorder.DisposeAsync();
        _cts.Dispose();
    }
}