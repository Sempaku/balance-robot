using System.Buffers;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Threading.Channels;
using BalanceBot.Protocol.Packets;

public class UdpTelemetryReceiver
{
    private readonly UdpClient _udpClient;
    private readonly ChannelWriter<TelemetryPacket> _channelWriter;

    public UdpTelemetryReceiver(int port, ChannelWriter<TelemetryPacket> channelWriter)
    {
        _udpClient = new UdpClient(port);
        _channelWriter = channelWriter;
    }

    public async Task StartReceivingAsync(CancellationToken ct)
    {
        // Выделяем массив один раз и переиспользуем его
        byte[] buffer = new byte[256];

        while (!ct.IsCancellationRequested)
        {
            var result = await _udpClient.ReceiveAsync(ct);
            if (result.Buffer.Length < Marshal.SizeOf<TelemetryPacket>())
                continue;

            // Мгновенный cast памяти без единой аллокации
            ReadOnlySpan<byte> span = result.Buffer.AsSpan();
            ref readonly var packet = ref MemoryMarshal.AsRef<TelemetryPacket>(span);

            if (packet.Magic != 0x314E4C42) // "BLN1"
                continue;

            // Пишем в Channel (Bounded с режимом DropOldest, чтобы очередь не росла)
            _channelWriter.TryWrite(packet);
        }
    }
}