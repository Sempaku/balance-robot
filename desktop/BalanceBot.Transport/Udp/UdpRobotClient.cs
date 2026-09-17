using System.Buffers;
using System.Net;
using System.Net.Sockets;
using System.Threading.Channels;
using BalanceBot.Protocol;
using BalanceBot.Protocol.Packets;
using BalanceBot.Protocol.Serialization;

namespace BalanceBot.Transport.Udp;

/// <summary>
/// UDP-клиент обмена данными с роботом в реальном времени (100–200 Гц).
/// Совмещает приём телеметрии, отправку keepalive и передачу команд управления на одном сокете.
/// </summary>
public sealed class UdpRobotClient : IAsyncDisposable
{
    private readonly UdpClient _socket;
    private readonly IPEndPoint _robotEndpoint;
    private readonly Channel<TelemetryPacket> _telemetryChannel;

    private readonly CancellationTokenSource _cts = new();
    private Task? _receiveTask;
    private Task? _keepAliveTask;

    private uint _driveSeq = 0;
    private uint _helloSeq = 0;

    public ulong TotalPacketsReceived { get; private set; }
    public ulong PacketsLost { get; private set; }
    public DateTime LastPacketReceivedAt { get; private set; } = DateTime.MinValue;
    public bool IsReceiving => (DateTime.UtcNow - LastPacketReceivedAt).TotalSeconds < 2.0;

    public ChannelReader<TelemetryPacket> TelemetryReader => _telemetryChannel.Reader;

    public UdpRobotClient(string robotIp, int port = ProtocolConstants.UdpTelemetryPort)
    {
        _robotEndpoint = new IPEndPoint(IPAddress.Parse(robotIp), port);
        _socket = new UdpClient(new IPEndPoint(IPAddress.Any, 0));

        _telemetryChannel = Channel.CreateBounded<TelemetryPacket>(new BoundedChannelOptions(200)
        {
            SingleWriter = true,
            SingleReader = false,
            FullMode = BoundedChannelFullMode.DropOldest
        });
    }

    public void Start()
    {
        _receiveTask = Task.Run(ReceiveLoopAsync);
        _keepAliveTask = Task.Run(KeepAliveLoopAsync);
    }

    /// <summary>
    /// Неблокирующая отправка команды хода и поворота со стиков геймпада / WASD (20 байт).
    /// </summary>
    public void SendDrive(float forward, float turn)
    {
        Span<byte> buffer = stackalloc byte[ProtocolConstants.DrivePacketSize];
        var packet = DrivePacket.Create(Interlocked.Increment(ref _driveSeq), forward, turn);
        BinaryPacketSerializer.WriteDrivePacket(buffer, in packet);

        try
        {
            _socket.Send(buffer, _robotEndpoint);
        }
        catch
        {
            // Пропуск одиночных сетевых сбоев
        }
    }

    private async Task ReceiveLoopAsync()
    {
        byte[] receiveBuffer = new byte[256];
        uint lastSequence = 0;
        bool hasLastSequence = false;

        while (!_cts.Token.IsCancellationRequested)
        {
            try
            {
                var result = await _socket.ReceiveAsync(_cts.Token);

                if (BinaryPacketSerializer.TryParseTelemetry(result.Buffer, out var packet))
                {
                    TotalPacketsReceived++;
                    LastPacketReceivedAt = DateTime.UtcNow;

                    // Переполнение uint обрабатывается самой арифметикой:
                    // разность считается по модулю 2^32.
                    if (hasLastSequence && packet.Sequence != lastSequence + 1)
                    {
                        PacketsLost += packet.Sequence - (lastSequence + 1);
                    }
                    lastSequence = packet.Sequence;
                    hasLastSequence = true;

                    _telemetryChannel.Writer.TryWrite(packet);
                }
            }
            catch (OperationCanceledException)
            {
                break;
            }
            catch (Exception)
            {
                await Task.Delay(50, _cts.Token);
            }
        }
    }

    private async Task KeepAliveLoopAsync()
    {
        byte[] helloBuffer = new byte[ProtocolConstants.HeaderSize];
        using var timer = new PeriodicTimer(TimeSpan.FromSeconds(1));

        while (!_cts.Token.IsCancellationRequested && await timer.WaitForNextTickAsync(_cts.Token))
        {
            try
            {
                BinaryPacketSerializer.WriteHelloPacket(helloBuffer, Interlocked.Increment(ref _helloSeq));
                await _socket.SendAsync(helloBuffer, helloBuffer.Length, _robotEndpoint);
            }
            catch (OperationCanceledException)
            {
                break;
            }
            catch
            {
                // Пропускаем сетевые сбои
            }
        }
    }

    public async ValueTask DisposeAsync()
    {
        _cts.Cancel();
        _socket.Close();

        if (_receiveTask != null) await Task.WhenAny(_receiveTask, Task.Delay(500));
        if (_keepAliveTask != null) await Task.WhenAny(_keepAliveTask, Task.Delay(500));

        _telemetryChannel.Writer.TryComplete();
        _socket.Dispose();
        _cts.Dispose();
    }
}