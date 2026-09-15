using System.Runtime.InteropServices;
using BalanceBot.Protocol.Packets;

namespace BalanceBot.Protocol.Serialization;

public static class BinaryPacketSerializer
{
    /// <summary>
    /// Парсинг сырого буфера UDP в структуру TelemetryPacket без аллокаций.
    /// </summary>
    public static bool TryParseTelemetry(ReadOnlySpan<byte> data, out TelemetryPacket packet)
    {
        if (data.Length < ProtocolConstants.TelemetryPacketSize)
        {
            packet = default;
            return false;
        }

        packet = MemoryMarshal.Read<TelemetryPacket>(data);

        // Проверяем сигнатуру и версию
        return packet.Magic == ProtocolConstants.Magic &&
               packet.Version == ProtocolConstants.Version &&
               packet.MsgType == ProtocolConstants.MsgTelemetry;
    }

    /// <summary>
    /// Сериализация DrivePacket в переданный спан байт (ровно 20 байт).
    /// </summary>
    public static void WriteDrivePacket(Span<byte> destination, in DrivePacket packet)
    {
        if (destination.Length < ProtocolConstants.DrivePacketSize)
            throw new ArgumentException("Буфер меньше ProtocolConstants.DrivePacketSize", nameof(destination));

        MemoryMarshal.Write(destination, in packet);
    }

    /// <summary>
    /// Создание пакета Discovery HELLO (12 байт) для поиска робота в эфире.
    /// </summary>
    public static void WriteHelloPacket(Span<byte> destination, uint sequence)
    {
        if (destination.Length < ProtocolConstants.HeaderSize)
            throw new ArgumentException("Буфер меньше ProtocolConstants.HeaderSize", nameof(destination));

        BitConverter.TryWriteBytes(destination[0..4], ProtocolConstants.Magic);
        destination[4] = ProtocolConstants.Version;
        destination[5] = ProtocolConstants.MsgHello;
        BitConverter.TryWriteBytes(destination[6..8], (ushort)0);
        BitConverter.TryWriteBytes(destination[8..12], sequence);
    }
}