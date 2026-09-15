namespace BalanceBot.Protocol;

public static class ProtocolConstants
{
    /// <summary>
    /// Сигнатура протокола "BLN1" (0x314E4C42 в little-endian).
    /// Отсекает чужие пакеты в локальной сети.
    /// </summary>
    public const uint Magic = 0x314E_4C42;

    /// <summary>
    /// Версия бинарного протокола.
    /// </summary>
    public const byte Version = 1;

    // --- Сетевые порты ---
    public const int UdpTelemetryPort = 5555;

    public const int TcpCommandPort = 5556;

    // --- Типы UDP сообщений ---
    public const byte MsgTelemetry = 1;

    public const byte MsgDrive = 2;
    public const byte MsgHello = 3;
    public const byte MsgInfo = 4;

    // --- Точные размеры бинарных пакетов ---
    public const int HeaderSize = 12;

    public const int TelemetryPacketSize = 72;
    public const int DrivePacketSize = 20;

    // --- Битовые флаги состояния (байт Flags в TelemetryPacket) ---
    public const byte FlagArmed = 1 << 0;     // Робот активен (балансирует)

    public const byte FlagOta = 1 << 1;       // В процессе приёма прошивки
    public const byte FlagImuFault = 1 << 2;  // Ошибка датчика MPU6050
}

/// <summary>
/// Состояние робота (поле State в TelemetryPacket).
/// </summary>
public enum RobotState : byte
{
    Idle = 0,
    Balancing = 1
}