using BalanceBot.Core.Telemetry;
using BalanceBot.Core.Tuning;
using BalanceBot.Protocol.Packets;
using BalanceBot.Transport.Tcp;
using BalanceBot.Transport.Udp;

namespace BalanceBot.Core;

/// <summary>
/// Активная сессия взаимодействия с подключенным роботом.
/// </summary>
public sealed class RobotSession : IAsyncDisposable
{
    public RobotInfo Info { get; }
    public UdpRobotClient Udp { get; }
    public TcpCommandClient Tcp { get; }
    public TelemetryHub Telemetry { get; }
    public ParameterManager Parameters { get; }

    public bool IsConnected => Tcp.IsConnected && Udp.IsReceiving;

    private RobotSession(RobotInfo info, UdpRobotClient udp, TcpCommandClient tcp)
    {
        Info = info;
        Udp = udp;
        Tcp = tcp;
        Telemetry = new TelemetryHub(udp.TelemetryReader);
        Parameters = new ParameterManager(tcp);
    }

    /// <summary>
    /// Подключение к роботу: поднимает UDP поток и открывает командный канал TCP.
    /// </summary>
    public static async Task<RobotSession> ConnectAsync(RobotInfo info, CancellationToken ct = default)
    {
        var udp = new UdpRobotClient(info.RemoteIpAddress);
        var tcp = new TcpCommandClient(info.RemoteIpAddress, info.CommandPort);

        try
        {
            // Подключаем TCP
            await tcp.ConnectAsync(ct);

            // Запускаем UDP приём телеметрии
            udp.Start();

            var session = new RobotSession(info, udp, tcp);
            session.Telemetry.Start();

            // Загружаем начальные метаданные параметров
            await session.Parameters.RefreshAllAsync(ct);

            return session;
        }
        catch
        {
            await udp.DisposeAsync();
            await tcp.DisposeAsync();
            throw;
        }
    }

    /// <summary>
    /// Отправка стиков геймпада (делегируется в UdpRobotClient).
    /// </summary>
    public void Drive(float forward, float turn)
    {
        Udp.SendDrive(forward, turn);
    }

    /// <summary>
    /// Включить балансировку (Arm).
    /// </summary>
    public Task<bool> ArmAsync(CancellationToken ct = default) => Tcp.ArmAsync(ct);

    /// <summary>
    /// Аварийный стоп (Disarm).
    /// </summary>
    public Task<bool> DisarmAsync(CancellationToken ct = default) => Tcp.DisarmAsync(ct);

    public async ValueTask DisposeAsync()
    {
        await Telemetry.DisposeAsync();
        await Udp.DisposeAsync();
        await Tcp.DisposeAsync();
    }
}