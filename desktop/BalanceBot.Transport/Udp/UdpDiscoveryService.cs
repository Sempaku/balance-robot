using System.Net;
using System.Net.NetworkInformation;
using System.Net.Sockets;
using BalanceBot.Protocol;
using BalanceBot.Protocol.Packets;
using BalanceBot.Protocol.Serialization;

namespace BalanceBot.Transport.Udp;

/// <summary>
/// Сервис обнаружения роботов в локальной сети через широковещательный UDP HELLO.
/// </summary>
public static class UdpDiscoveryService
{
    /// <summary>
    /// Ищет роботов в сети в течение указанного времени.
    /// </summary>
    /// <param name="timeout">Время сканирования (рекомендуется 1-2 секунды).</param>
    /// <param name="ct">Токен отмены.</param>
    /// <returns>Список найденных роботов.</returns>
    public static async Task<IReadOnlyList<RobotInfo>> DiscoverRobotsAsync(TimeSpan timeout, CancellationToken ct = default)
    {
        // ВАЖНО: Привязываем сокет к IPAddress.Any и порту 0 (любой свободный порт ОС).
        // Это гарантирует, что IsBound == true и ReceiveAsync не падает с ошибкой Bind.
        using var client = new UdpClient(new IPEndPoint(IPAddress.Any, 0));
        client.EnableBroadcast = true;

        var discovered = new Dictionary<string, RobotInfo>();
        var broadcastEndpoints = GetBroadcastAddresses();

        // Буфер для отправки HELLO (12 байт)
        byte[] helloBuffer = new byte[ProtocolConstants.HeaderSize];
        BinaryPacketSerializer.WriteHelloPacket(helloBuffer, sequence: 1);

        using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        cts.CancelAfter(timeout);

        // Фоновая задача отправки нескольких запросов с интервалом (для надёжности в Wi-Fi эфире)
        var sendTask = Task.Run(async () =>
        {
            try
            {
                for (int i = 0; i < 3 && !cts.Token.IsCancellationRequested; i++)
                {
                    foreach (var endpoint in broadcastEndpoints)
                    {
                        await client.SendAsync(helloBuffer, helloBuffer.Length, endpoint);
                    }
                    await Task.Delay(250, cts.Token);
                }
            }
            catch (OperationCanceledException) { }
        }, cts.Token);

        // Читаем ответы робота до истечения таймаута
        try
        {
            while (!cts.Token.IsCancellationRequested)
            {
                var result = await client.ReceiveAsync(cts.Token);
                string remoteIp = result.RemoteEndPoint.Address.ToString();

                if (RobotInfo.TryParse(result.Buffer, remoteIp, out var info) && info != null)
                {
                    discovered[info.RemoteIpAddress] = info;
                }
            }
        }
        catch (OperationCanceledException) { }

        await sendTask;
        return discovered.Values.ToList();
    }

    /// <summary>
    /// Собирает список широковещательных адресов (общий 255.255.255.255 + направленные для каждой подсети).
    /// </summary>
    private static List<IPEndPoint> GetBroadcastAddresses()
    {
        var list = new List<IPEndPoint>
        {
            new(IPAddress.Broadcast, ProtocolConstants.UdpTelemetryPort)
        };

        try
        {
            foreach (var iface in NetworkInterface.GetAllNetworkInterfaces())
            {
                if (iface.OperationalStatus != OperationalStatus.Up ||
                    iface.NetworkInterfaceType == NetworkInterfaceType.Loopback)
                    continue;

                foreach (var unicast in iface.GetIPProperties().UnicastAddresses)
                {
                    if (unicast.Address.AddressFamily != AddressFamily.InterNetwork || unicast.IPv4Mask == null)
                        continue;

                    byte[] ipBytes = unicast.Address.GetAddressBytes();
                    byte[] maskBytes = unicast.IPv4Mask.GetAddressBytes();
                    byte[] broadcastBytes = new byte[4];

                    for (int i = 0; i < 4; i++)
                    {
                        broadcastBytes[i] = (byte)(ipBytes[i] | ~maskBytes[i]);
                    }

                    list.Add(new IPEndPoint(new IPAddress(broadcastBytes), ProtocolConstants.UdpTelemetryPort));
                }
            }
        }
        catch
        {
            // Если нет прав на чтение интерфейсов, общего broadcast 255.255.255.255 достаточно
        }

        return list;
    }
}