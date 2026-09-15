using System.Runtime.InteropServices;

namespace BalanceBot.Protocol.Packets;

[StructLayout(LayoutKind.Sequential, Pack = 1)]
public struct TelemetryPacket
{
    public uint Magic;          // 0x314E4C42 ("BLN1")
    public byte Version;        // 1
    public byte MsgType;        // 1
    public ushort PayloadLen;   // 60
    public uint Sequence;
    public uint UptimeMs;
    public byte State;          // 0 = Idle, 1 = Balancing
    public byte Flags;          // Bit 0: Armed, Bit 1: OTA, Bit 2: IMU Fault
    public ushort ImuErrors;

    public float Angle;
    public float AccelAngle;
    public float Gyro;
    public float AccelMag;
    public float GyroBias;
    public float Trim;

    public float Cmd;
    public float WheelRate;
    public float TermP;
    public float TermLean;
    public float TermD;

    public int Steps;
    public ushort DtUs;
    public ushort LoopUs;
}