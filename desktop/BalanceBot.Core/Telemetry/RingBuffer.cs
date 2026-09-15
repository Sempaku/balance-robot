namespace BalanceBot.Core.Telemetry;

/// <summary>
/// Потокобезопасный кольцевой буфер фиксированной емкости для хранения истории телеметрии.
/// При переполнении старые элементы перезаписываются.
/// </summary>
public sealed class RingBuffer<T>
{
    private readonly T[] _buffer;
    private readonly int _capacity;
    private int _head;
    private int _count;
    private readonly object _lock = new();

    public int Capacity => _capacity;
    public int Count
    { get { lock (_lock) return _count; } }

    public RingBuffer(int capacity)
    {
        if (capacity <= 0)
            throw new ArgumentOutOfRangeException(nameof(capacity), "Емкость буфера должна быть > 0");

        _capacity = capacity;
        _buffer = new T[capacity];
    }

    /// <summary>
    /// Добавляет элемент в кольцевой буфер.
    /// </summary>
    public void Push(in T item)
    {
        lock (_lock)
        {
            _buffer[_head] = item;
            _head = (_head + 1) % _capacity;
            if (_count < _capacity)
            {
                _count++;
            }
        }
    }

    /// <summary>
    /// Копирует текущее содержимое буфера в массив в хронологическом порядке (от старых к новым).
    /// </summary>
    public T[] ToArray()
    {
        lock (_lock)
        {
            if (_count == 0) return Array.Empty<T>();

            var result = new T[_count];
            int start = (_head - _count + _capacity) % _capacity;

            if (start + _count <= _capacity)
            {
                Array.Copy(_buffer, start, result, 0, _count);
            }
            else
            {
                int firstChunk = _capacity - start;
                Array.Copy(_buffer, start, result, 0, firstChunk);
                Array.Copy(_buffer, 0, result, firstChunk, _count - firstChunk);
            }

            return result;
        }
    }

    /// <summary>
    /// Очищает буфер.
    /// </summary>
    public void Clear()
    {
        lock (_lock)
        {
            _head = 0;
            _count = 0;
            Array.Clear(_buffer);
        }
    }
}