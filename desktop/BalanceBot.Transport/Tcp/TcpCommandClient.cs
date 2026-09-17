using System.IO;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using BalanceBot.Protocol;
using BalanceBot.Protocol.Dtos;

namespace BalanceBot.Transport.Tcp;

/// <summary>
/// TCP-клиент управления настройками робота с поддержкой автопереподключения и фонового Keep-Alive.
/// </summary>
public sealed class TcpCommandClient : IAsyncDisposable
{
    private readonly string _host;
    private readonly int _port;
    private TcpClient? _client;
    private NetworkStream? _stream;
    private StreamReader? _reader;
    private readonly SemaphoreSlim _lock = new(1, 1);

    private readonly CancellationTokenSource _cts = new();
    private Task? _keepAliveTask;

    public bool IsConnected => _client?.Connected == true;

    public TcpCommandClient(string host, int port = ProtocolConstants.TcpCommandPort)
    {
        _host = host;
        _port = port;
    }

    /// <summary>
    /// Подключение к роботу и запуск фонового пинга сессии.
    /// </summary>
    public async Task ConnectAsync(CancellationToken ct = default)
    {
        await _lock.WaitAsync(ct);
        try
        {
            await EnsureConnectedInternalAsync(ct);

            // Запускаем фоновый пинг каждые 20 секунд (ESP32 закрывает сокет через 60 сек простоя)
            _keepAliveTask ??= Task.Run(KeepAliveLoopAsync);
        }
        finally
        {
            _lock.Release();
        }
    }

    private async Task EnsureConnectedInternalAsync(CancellationToken ct)
    {
        if (_client?.Connected == true && _stream != null && _reader != null)
        {
            return;
        }

        CleanupSocket();

        _client = new TcpClient
        {
            NoDelay = true,
            ReceiveTimeout = 5000,
            SendTimeout = 5000
        };

        _client.Client.SetSocketOption(SocketOptionLevel.Socket, SocketOptionName.KeepAlive, true);

        await _client.ConnectAsync(_host, _port, ct);
        _stream = _client.GetStream();
        _reader = new StreamReader(_stream, Encoding.UTF8);
    }

    private void CleanupSocket()
    {
        try { _reader?.Dispose(); } catch { }
        try { _stream?.Dispose(); } catch { }
        try { _client?.Dispose(); } catch { }
        _reader = null;
        _stream = null;
        _client = null;
    }

    /// <summary>
    /// Отправка команды с автопереподключением при разрыве сокета.
    /// </summary>
    public async Task<string> SendRawCommandAsync(object request, CancellationToken ct = default)
    {
        await _lock.WaitAsync(ct);
        try
        {
            string json = JsonSerializer.Serialize(request) + "\n";
            byte[] bytes = Encoding.UTF8.GetBytes(json);

            // 1-я попытка отправки
            try
            {
                await EnsureConnectedInternalAsync(ct);

                await _stream!.WriteAsync(bytes, ct);
                await _stream.FlushAsync(ct);

                string? response = await _reader!.ReadLineAsync(ct);
                if (response != null)
                {
                    return response;
                }
            }
            catch (Exception)
            {
                // Сокет мог протухнуть — пробуем переподключиться ниже
            }

            // 2-я попытка: переподключение и повторная отправка
            CleanupSocket();
            await EnsureConnectedInternalAsync(ct);

            await _stream!.WriteAsync(bytes, ct);
            await _stream.FlushAsync(ct);

            string? retryResponse = await _reader!.ReadLineAsync(ct);
            return retryResponse ?? throw new EndOfStreamException("Робот разорвал соединение.");
        }
        finally
        {
            _lock.Release();
        }
    }

    /// <summary>
    /// Фоновый пинг сессии каждые 20 секунд для предотвращения IDLE-таймаута ESP32.
    /// </summary>
    private async Task KeepAliveLoopAsync()
    {
        using var timer = new PeriodicTimer(TimeSpan.FromSeconds(20));

        while (!_cts.Token.IsCancellationRequested && await timer.WaitForNextTickAsync(_cts.Token))
        {
            try
            {
                await GetSystemInfoAsync(_cts.Token);
            }
            catch
            {
                // Пропускаем сетевые сбои в фоновом пинге
            }
        }
    }

    // =========================================================================
    // Типизированные команды робота
    // =========================================================================

    public async Task<RobotSystemInfo?> GetSystemInfoAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "info" }, ct);
        return JsonSerializer.Deserialize<RobotSystemInfo>(res);
    }

    public async Task<ParamCatalogResponse?> GetParamCatalogAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "param_catalog" }, ct);
        return JsonSerializer.Deserialize<ParamCatalogResponse>(res);
    }

    public async Task<Dictionary<string, float>> GetParamsAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "get_params" }, ct);
        using var doc = JsonDocument.Parse(res);
        var result = new Dictionary<string, float>();

        if (doc.RootElement.TryGetProperty("params", out var paramsObj))
        {
            foreach (var prop in paramsObj.EnumerateObject())
            {
                result[prop.Name] = (float)prop.Value.GetDouble();
            }
        }
        return result;
    }

    public async Task<bool> SetParamsAsync(Dictionary<string, float> parameters, CancellationToken ct = default)
    {
        var dict = new Dictionary<string, object> { ["cmd"] = "set_params" };
        foreach (var (k, v) in parameters) dict[k] = v;

        string res = await SendRawCommandAsync(dict, ct);
        using var doc = JsonDocument.Parse(res);
        return doc.RootElement.TryGetProperty("ok", out var okProp) && okProp.GetBoolean();
    }

    public async Task<bool> SaveParamsToNvsAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "save_params" }, ct);
        using var doc = JsonDocument.Parse(res);
        return doc.RootElement.TryGetProperty("ok", out var okProp) && okProp.GetBoolean();
    }

    public async Task<bool> ResetParamsAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "reset_params" }, ct);
        using var doc = JsonDocument.Parse(res);
        return doc.RootElement.TryGetProperty("ok", out var okProp) && okProp.GetBoolean();
    }

    public async Task<bool> ArmAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "arm" }, ct);
        using var doc = JsonDocument.Parse(res);
        return doc.RootElement.TryGetProperty("ok", out var okProp) && okProp.GetBoolean();
    }

    public async Task<bool> DisarmAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "disarm" }, ct);
        using var doc = JsonDocument.Parse(res);
        return doc.RootElement.TryGetProperty("ok", out var okProp) && okProp.GetBoolean();
    }

    /// <summary>
    /// Запуск автонастройки PID на роботе (релейный тест Зиглера-Николса).
    /// Робот должен быть выключен из баланса и удерживаться вертикально.
    /// </summary>
    public async Task<bool> StartAutotuneAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "autotune" }, ct);
        using var doc = JsonDocument.Parse(res);
        return doc.RootElement.TryGetProperty("ok", out var okProp) && okProp.GetBoolean();
    }

    /// <summary>
    /// Текущий статус автонастройки: idle / running / done / failed + результат.
    /// </summary>
    public async Task<AutotuneStatus?> GetAutotuneStatusAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "autotune_status" }, ct);
        return JsonSerializer.Deserialize<AutotuneStatus>(res);
    }

    public async Task<WifiListResponse?> GetWifiNetworksAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "get_wifi" }, ct);
        return JsonSerializer.Deserialize<WifiListResponse>(res);
    }

    public async Task<bool> SetWifiNetworkAsync(string ssid, string psk, int priority, CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "set_wifi", ssid, psk, priority }, ct);
        using var doc = JsonDocument.Parse(res);
        return doc.RootElement.TryGetProperty("ok", out var okProp) && okProp.GetBoolean();
    }

    public async Task<bool> DeleteWifiNetworkAsync(string ssid, CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "del_wifi", ssid }, ct);
        using var doc = JsonDocument.Parse(res);
        return doc.RootElement.TryGetProperty("ok", out var okProp) && okProp.GetBoolean();
    }

    public async Task<BlackBoxResponse?> GetBlackBoxAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "get_blackbox" }, ct);
        return JsonSerializer.Deserialize<BlackBoxResponse>(res);
    }

    public async Task<bool> ClearBlackBoxAsync(CancellationToken ct = default)
    {
        string res = await SendRawCommandAsync(new { cmd = "clear_blackbox" }, ct);
        using var doc = JsonDocument.Parse(res);
        return doc.RootElement.TryGetProperty("ok", out var okProp) && okProp.GetBoolean();
    }

    public async Task RebootAsync(CancellationToken ct = default)
    {
        try
        {
            await SendRawCommandAsync(new { cmd = "reboot" }, ct);
        }
        catch
        {
            // Перезагрузка рвет связь намеренно
        }
    }

    public async ValueTask DisposeAsync()
    {
        _cts.Cancel();
        if (_keepAliveTask != null)
        {
            await Task.WhenAny(_keepAliveTask, Task.Delay(500));
        }

        CleanupSocket();
        _lock.Dispose();
        _cts.Dispose();
    }
}