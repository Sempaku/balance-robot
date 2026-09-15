using System.Text;

namespace BalanceBot.Protocol.Packets;

public record RobotInfo(string Name, string FirmwareVersion, int CommandPort, string RemoteIpAddress)
{
    /// <summary>
    /// Человекочитаемое имя для выпадающего списка (например: "BalanceBot-3A1F (192.168.1.45)").
    /// </summary>
    public string DisplayName => $"{Name} ({RemoteIpAddress})";

    public override string ToString() => DisplayName;

    /// <summary>
    /// Парсит ответ MSG_INFO (тип 4) от робота на UDP discovery.
    /// </summary>
    public static bool TryParse(ReadOnlySpan<byte> data, string remoteIp, out RobotInfo? info)
    {
        info = null;

        if (data.Length < ProtocolConstants.HeaderSize + 4)
            return false;

        uint magic = BitConverter.ToUInt32(data[..4]);
        byte version = data[4];
        byte kind = data[5];

        if (magic != ProtocolConstants.Magic || version != ProtocolConstants.Version || kind != ProtocolConstants.MsgInfo)
            return false;

        int offset = ProtocolConstants.HeaderSize;
        ushort commandPort = BitConverter.ToUInt16(data.Slice(offset, 2));
        offset += 2;

        byte nameLen = data[offset++];
        if (offset + nameLen > data.Length) return false;
        string name = Encoding.UTF8.GetString(data.Slice(offset, nameLen));
        offset += nameLen;

        if (offset >= data.Length) return false;
        byte verLen = data[offset++];
        if (offset + verLen > data.Length) return false;
        string firmware = Encoding.UTF8.GetString(data.Slice(offset, verLen));

        info = new RobotInfo(name, firmware, commandPort, remoteIp);
        return true;
    }
}