using System.Text.Json.Serialization;

namespace BalanceBot.Protocol.Dtos;

public record RobotSystemInfo(
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("firmware")] string Firmware,
    [property: JsonPropertyName("slot")] string RunningSlot,
    [property: JsonPropertyName("mode")] string Mode,
    [property: JsonPropertyName("ssid")] string Ssid,
    [property: JsonPropertyName("ip")] string Ip
);

public record ParamCatalogItem(
    [property: JsonPropertyName("key")] string Key,
    [property: JsonPropertyName("unit")] string Unit,
    [property: JsonPropertyName("min")] float Min,
    [property: JsonPropertyName("max")] float Max,
    [property: JsonPropertyName("default")] float DefaultValue,
    [property: JsonPropertyName("about")] string About
);

public record ParamCatalogResponse(
    [property: JsonPropertyName("ok")] bool Ok,
    [property: JsonPropertyName("params")] List<ParamCatalogItem> Params
);

// Добавлено свойство IsActive
public record WifiNetworkDto(
    [property: JsonPropertyName("ssid")] string Ssid,
    [property: JsonPropertyName("priority")] int Priority,
    [property: JsonPropertyName("active")] bool IsActive = false
);

public record WifiListResponse(
    [property: JsonPropertyName("ok")] bool Ok,
    [property: JsonPropertyName("networks")] List<WifiNetworkDto> Networks
);

public record BlackBoxFrameDto(
    [property: JsonPropertyName("t_ms")] uint TimestampMs,
    [property: JsonPropertyName("angle")] float Angle,
    [property: JsonPropertyName("gyro")] float Gyro,
    [property: JsonPropertyName("cmd")] float Cmd,
    [property: JsonPropertyName("wheel_rate")] float WheelRate,
    [property: JsonPropertyName("steps")] int Steps,
    [property: JsonPropertyName("dt_us")] ushort DtUs
);

public record BlackBoxResponse(
    [property: JsonPropertyName("ok")] bool Ok,
    [property: JsonPropertyName("frames")] List<BlackBoxFrameDto> Frames
);