using System.IO;
using System.Text.Json;
using BalanceBot.Protocol.Dtos;
using BalanceBot.Transport.Tcp;

namespace BalanceBot.Core.Tuning;

public sealed class ParameterManager
{
    private readonly TcpCommandClient _tcpClient;

    /// <summary>
    /// Метаданные коэффициентов (диапазоны, единицы измерения, подсказки), полученные от робота.
    /// </summary>
    public List<ParamCatalogItem> Catalog { get; private set; } = new();

    /// <summary>
    /// Текущие актуальные значения в оперативной памяти робота.
    /// </summary>
    public Dictionary<string, float> CurrentValues { get; } = new();

    /// <summary>
    /// Значения, отредактированные в UI, но еще не примененные к роботу.
    /// </summary>
    public Dictionary<string, float> ModifiedValues { get; } = new();

    public bool HasUnsavedChanges => ModifiedValues.Count > 0;

    public ParameterManager(TcpCommandClient tcpClient)
    {
        _tcpClient = tcpClient;
    }

    /// <summary>
    /// Первичная синхронизация: загрузка каталога и текущих коэффициентов с робота.
    /// </summary>
    public async Task RefreshAllAsync(CancellationToken ct = default)
    {
        // 1. Запрашиваем каталог метаданных
        var catalogRes = await _tcpClient.GetParamCatalogAsync(ct);
        if (catalogRes?.Ok == true && catalogRes.Params != null)
        {
            Catalog = catalogRes.Params;
        }

        // 2. Запрашиваем текущие значения
        var values = await _tcpClient.GetParamsAsync(ct);
        CurrentValues.Clear();
        ModifiedValues.Clear();
        foreach (var (k, v) in values)
        {
            CurrentValues[k] = v;
        }
    }

    /// <summary>
    /// Изменение параметра пользователем в UI (подвинул слайдер).
    /// </summary>
    public void SetLocal(string key, float value)
    {
        if (CurrentValues.TryGetValue(key, out float current) && Math.Abs(current - value) < 0.0001f)
        {
            ModifiedValues.Remove(key);
        }
        else
        {
            ModifiedValues[key] = value;
        }
    }

    /// <summary>
    /// Применить все измененные параметры на робота немедленно (в RAM).
    /// </summary>
    public async Task<bool> ApplyModifiedAsync(CancellationToken ct = default)
    {
        if (ModifiedValues.Count == 0) return true;

        bool success = await _tcpClient.SetParamsAsync(ModifiedValues, ct);
        if (success)
        {
            foreach (var (k, v) in ModifiedValues)
            {
                CurrentValues[k] = v;
            }
            ModifiedValues.Clear();
        }
        return success;
    }

    /// <summary>
    /// Зафиксировать текущие коэффициенты в энергонезависимой памяти робота NVS.
    /// </summary>
    public async Task<bool> SaveToNvsAsync(CancellationToken ct = default)
    {
        // Сначала применяем несохраненные правки
        if (ModifiedValues.Count > 0)
        {
            await ApplyModifiedAsync(ct);
        }
        return await _tcpClient.SaveParamsToNvsAsync(ct);
    }

    /// <summary>
    /// Сброс всех коэффициентов к заводским значениям, зашитым в прошивке.
    /// </summary>
    public async Task<bool> ResetToDefaultsAsync(CancellationToken ct = default)
    {
        bool success = await _tcpClient.ResetParamsAsync(ct);
        if (success)
        {
            await RefreshAllAsync(ct);
        }
        return success;
    }

    // =========================================================================
    // Профили настроек на диске ПК (JSON файлы)
    // =========================================================================

    public async Task ExportProfileToFileAsync(string filePath)
    {
        string json = JsonSerializer.Serialize(CurrentValues, new JsonSerializerOptions { WriteIndented = true });
        await File.WriteAllTextAsync(filePath, json);
    }

    public async Task<bool> ImportProfileFromFileAsync(string filePath, CancellationToken ct = default)
    {
        if (!File.Exists(filePath)) return false;

        string json = await File.ReadAllTextAsync(filePath, ct);
        var imported = JsonSerializer.Deserialize<Dictionary<string, float>>(json);
        if (imported == null) return false;

        bool success = await _tcpClient.SetParamsAsync(imported, ct);
        if (success)
        {
            await RefreshAllAsync(ct);
        }
        return success;
    }
}