using System.Runtime.InteropServices;

namespace BalanceBot.Protocol.Packets;

[StructLayout(LayoutKind.Sequential, Pack = 1)]
public struct DrivePacket
{
    // === Заголовок протокола (12 байт) ===
    public uint Magic;            // ProtocolConstants.Magic

    public byte Version;          // 1
    public byte MsgType;          // 2 = MSG_DRIVE
    public ushort PayloadLen;     // 8 байт
    public uint Sequence;         // Инкрементный номер команды

    // === Полезная нагрузка (8 байт) ===
    public float Forward;         // Скорость/наклон: -1.0 (назад) .. +1.0 (вперед)

    public float Turn;            // Скорость разворота: -1.0 (влево) .. +1.0 (вправо)

    public static DrivePacket Create(uint sequence, float forward, float turn)
    {
        return new DrivePacket
        {
            Magic = ProtocolConstants.Magic,
            Version = ProtocolConstants.Version,
            MsgType = ProtocolConstants.MsgDrive,
            PayloadLen = 8,
            Sequence = sequence,
            Forward = Math.Clamp(forward, -1.0f, 1.0f),
            Turn = Math.Clamp(turn, -1.0f, 1.0f)
        };
    }
}