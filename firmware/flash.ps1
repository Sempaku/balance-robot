# Раннер для `cargo run`.
#
# Сначала пробует прошить робота по воздуху: находит его в сети
# широковещательным HELLO и заливает образ в свободный OTA-слот.
# Если робота в сети нет, откатывается на прошивку через USB — а на этой плате
# она требует ручного входа в режим загрузки (кнопки BOOT/EN), потому что
# линия DTR→GPIO0 в схеме автосброса не работает.
#
# Переменные окружения:
#   ROBOT_IP       — адрес робота, если не хочется ждать поиск
#   ROBOT_USB      — «1», чтобы сразу шить по USB, минуя OTA
#   ESPFLASH_PORT  — принудительный COM-порт

param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Binary,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$ErrorActionPreference = 'Stop'

$ProtocolMagic = 0x314E4C42
$ProtocolVersion = 1
$MsgHello = 3
$MsgInfo = 4
$TelemetryPort = 5555
# Одна проба занимает ~5 с (таймаут подключения espflash), так что это ~1.5 минуты
$AttemptLimit = 18

function New-HelloPacket {
    # Заголовок протокола: magic, версия, тип, длина полезной части, номер
    $packet = New-Object byte[] 12
    [BitConverter]::GetBytes([uint32]$ProtocolMagic).CopyTo($packet, 0)
    $packet[4] = $ProtocolVersion
    $packet[5] = $MsgHello
    return $packet
}

function Get-BroadcastAddresses {
    $addresses = @([System.Net.IPAddress]::Broadcast)
    # Направленный broadcast по каждой подсети: на некоторых адаптерах
    # 255.255.255.255 не уходит дальше самого хоста.
    Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        Where-Object { $_.IPAddress -ne '127.0.0.1' -and $_.PrefixLength -lt 31 } |
        ForEach-Object {
            $ip = [System.Net.IPAddress]::Parse($_.IPAddress).GetAddressBytes()
            $prefix = $_.PrefixLength
            # Маску собираем побайтно: сдвиги в PowerShell знаковые, и
            # 0xFFFFFFFF -shl n превращается в −1 ещё до приведения к uint32.
            $broadcast = New-Object byte[] 4
            for ($i = 0; $i -lt 4; $i++) {
                $bits = [Math]::Max(0, [Math]::Min(8, $prefix - $i * 8))
                $hostBits = [byte](256 - [Math]::Pow(2, 8 - $bits)) -bxor 0xFF
                $broadcast[$i] = [byte]($ip[$i] -bor $hostBits)
            }
            $addresses += [System.Net.IPAddress]::new($broadcast)
        }
    return $addresses | Select-Object -Unique
}

function Read-LengthPrefixedString($bytes, [ref]$offset) {
    if ($offset.Value -ge $bytes.Length) { return '' }
    $len = $bytes[$offset.Value]
    $offset.Value++
    if ($offset.Value + $len -gt $bytes.Length) { return '' }
    $text = [System.Text.Encoding]::UTF8.GetString($bytes, $offset.Value, $len)
    $offset.Value += $len
    return $text
}

function Find-Robot {
    if ($env:ROBOT_IP) {
        return @{ Ip = $env:ROBOT_IP; Name = 'from ROBOT_IP'; Version = '?'; Port = 5556 }
    }

    $client = New-Object System.Net.Sockets.UdpClient
    try {
        $client.EnableBroadcast = $true
        $client.Client.ReceiveTimeout = 700
        $hello = New-HelloPacket
        $targets = Get-BroadcastAddresses

        foreach ($attempt in 1..3) {
            foreach ($target in $targets) {
                $endpoint = New-Object System.Net.IPEndPoint($target, $TelemetryPort)
                try { [void]$client.Send($hello, $hello.Length, $endpoint) } catch { }
            }

            $from = New-Object System.Net.IPEndPoint([System.Net.IPAddress]::Any, 0)
            try {
                $data = $client.Receive([ref]$from)
            } catch {
                continue
            }

            if ($data.Length -lt 14) { continue }
            if ([BitConverter]::ToUInt32($data, 0) -ne [uint32]$ProtocolMagic) { continue }
            if ($data[4] -ne $ProtocolVersion -or $data[5] -ne $MsgInfo) { continue }

            $offset = 14
            $commandPort = [BitConverter]::ToUInt16($data, 12)
            $name = Read-LengthPrefixedString $data ([ref]$offset)
            $version = Read-LengthPrefixedString $data ([ref]$offset)

            return @{
                Ip      = $from.Address.ToString()
                Name    = $name
                Version = $version
                Port    = $commandPort
            }
        }
    } finally {
        $client.Close()
    }

    return $null
}

function Read-JsonLine($stream) {
    $bytes = New-Object System.Collections.Generic.List[byte]
    while ($true) {
        $b = $stream.ReadByte()
        if ($b -lt 0) { throw 'robot closed the connection' }
        if ($b -eq 10) { break }
        $bytes.Add([byte]$b)
    }
    $text = [System.Text.Encoding]::UTF8.GetString($bytes.ToArray())
    return $text | ConvertFrom-Json
}

function Send-JsonLine($stream, $object) {
    $text = ($object | ConvertTo-Json -Compress) + "`n"
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($text)
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush()
}

function Build-Image($binary) {
    $image = Join-Path $PSScriptRoot 'target\ota-image.bin'
    $table = Join-Path $PSScriptRoot 'partitions.csv'

    # espflash пишет в stderr даже при успехе, а под 'Stop' PowerShell считает
    # это фатальным. Успех определяем по коду возврата, а не по молчанию.
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & espflash save-image --chip esp32 --partition-table $table $binary $image 2>&1 | Out-Null
    $ErrorActionPreference = $previous

    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $image)) {
        throw "could not build image (espflash save-image = $LASTEXITCODE)"
    }
    return $image
}

function Send-OtaImage($robot, $imagePath) {
    $bytes = [System.IO.File]::ReadAllBytes($imagePath)
    $client = New-Object System.Net.Sockets.TcpClient

    try {
        $client.Connect($robot.Ip, $robot.Port)
        $stream = $client.GetStream()
        # Запись во флеш идёт медленнее сети, поэтому ответа ждём долго
        $stream.ReadTimeout = 60000
        $stream.WriteTimeout = 30000

        Send-JsonLine $stream @{ cmd = 'ota'; size = $bytes.Length }
        $ready = Read-JsonLine $stream
        if (-not $ready.ok) {
            throw "robot refused the image: $($ready.error)"
        }

        $chunk = 4096
        $sent = 0
        while ($sent -lt $bytes.Length) {
            $size = [Math]::Min($chunk, $bytes.Length - $sent)
            $stream.Write($bytes, $sent, $size)
            $sent += $size
            Write-Progress -Activity 'OTA update' `
                -Status "$([Math]::Round($sent / 1KB)) of $([Math]::Round($bytes.Length / 1KB)) KB" `
                -PercentComplete ($sent * 100 / $bytes.Length)
        }
        $stream.Flush()
        Write-Progress -Activity 'OTA update' -Completed

        $result = Read-JsonLine $stream
        if (-not $result.ok) {
            throw "robot rejected the image: $($result.error)"
        }
    } finally {
        $client.Close()
    }
}

function Get-EspPort {
    if ($env:ESPFLASH_PORT) { return $env:ESPFLASH_PORT }

    # Windows помнит все адаптеры, которые когда-либо втыкали, и показывает их
    # со статусом Unknown. Живым считаем только порт, который есть и в списке
    # устройств, и среди реально открытых системой.
    $live = [System.IO.Ports.SerialPort]::GetPortNames()
    $known = @()
    $ports = @()

    Get-PnpDevice -Class Ports -ErrorAction SilentlyContinue |
        Where-Object { $_.FriendlyName -match 'CH340|CP210|FTDI|USB-SERIAL|UART' } |
        ForEach-Object {
            if ($_.FriendlyName -match '\((COM\d+)\)') {
                $known += $Matches[1]
                if ($live -contains $Matches[1]) { $ports += $Matches[1] }
            }
        }

    if ($ports.Count -eq 0) {
        Write-Host ''
        Write-Host 'No live USB-serial port found.' -ForegroundColor Red
        if ($known.Count -gt 0) {
            Write-Host "Windows remembers $($known -join ', '), but none of them is present now." -ForegroundColor Red
        }
        Write-Host 'Plug the board into USB (data cable, not charge-only) and try again.' -ForegroundColor Red
        Write-Host 'Or set $env:ESPFLASH_PORT if you know the port name.' -ForegroundColor DarkGray
        return $null
    }

    if ($ports.Count -gt 1) {
        Write-Host "Multiple ports: $($ports -join ', '). Using $($ports[0]); override with `$env:ESPFLASH_PORT" -ForegroundColor Yellow
    }
    return $ports[0]
}

function Clear-StuckEspflash {
    Get-Process espflash -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

function Invoke-UsbFlash($binary, $rest) {
    Clear-StuckEspflash
    $port = Get-EspPort

    Write-Host ''
    Write-Host "Port: $port" -ForegroundColor Cyan
    Write-Host 'Enter download mode: hold BOOT -> press EN -> release BOOT' -ForegroundColor Yellow
    Write-Host 'Flashing starts automatically as soon as the board responds.' -ForegroundColor Yellow
    Write-Host ''

    # espflash пишет предупреждения в stderr, а под $ErrorActionPreference='Stop'
    # PowerShell считает это фатальным и рвёт скрипт на первой же попытке.
    $ErrorActionPreference = 'Continue'

    # Сначала тихо ждём, пока плата окажется в режиме загрузки: пробуем только
    # читать board-info, причём без сброса после — иначе сами же выбьем её обратно.
    $probeArgs = @('board-info', '--port', $port, '--before', 'no-reset', '--after', 'no-reset')
    $ready = $false

    for ($i = 1; $i -le $AttemptLimit; $i++) {
        & espflash @probeArgs *> $null
        if ($LASTEXITCODE -eq 0) { $ready = $true; break }
        if ($i % 4 -eq 1) {
            Write-Host 'Waiting for download mode... (hold BOOT -> press EN -> release BOOT)' -ForegroundColor DarkGray
        }
        Start-Sleep -Milliseconds 500
    }

    if (-not $ready) {
        Write-Host ''
        Write-Host 'Board never entered download mode.' -ForegroundColor Red
        Write-Host 'Sequence: hold BOOT, press and release EN, then release BOOT.' -ForegroundColor Red
        return 1
    }

    # Без явной таблицы разделов espflash возьмёт свою стандартную (single factory)
    # и OTA-слоты просто не появятся на флеше.
    $table = Join-Path $PSScriptRoot 'partitions.csv'
    # --after no-reset: сброс после записи идёт по той же нерабочей линии
    # DTR/RTS, и espflash на нём подвисает, так и не запустив монитор.
    $flashArgs = @('flash', '--port', $port, '--before', 'no-reset', '--after', 'no-reset',
                   '--partition-table', $table, $binary) + $rest
    & espflash @flashArgs
    if ($LASTEXITCODE -ne 0) { return $LASTEXITCODE }

    Write-Host ''
    Write-Host 'Flashed. Press EN once to start the board, monitor follows.' -ForegroundColor Green
    Write-Host ''
    # Свой монитор вместо espflash: тот перед чтением пытается связаться с
    # загрузчиком через DTR/RTS и на этой плате падает с «Failed to connect».
    # Запускаем в текущем процессе: вывод дочернего PowerShell осел бы в буфере
    # конвейера, и лог появился бы только после выхода из монитора.
    & (Join-Path $PSScriptRoot 'monitor.ps1') -Port $port
    return $LASTEXITCODE
}

if ($env:ROBOT_USB -ne '1') {
    Write-Host 'Looking for the robot on the network...' -ForegroundColor Cyan
    $robot = $null
    try { $robot = Find-Robot } catch { Write-Host "Discovery failed: $_" -ForegroundColor DarkGray }

    if ($robot) {
        Write-Host "Found $($robot.Name) at $($robot.Ip), firmware $($robot.Version)" -ForegroundColor Green
        try {
            $image = Build-Image $Binary
            Send-OtaImage $robot $image
            Write-Host 'Done: the robot is rebooting into the new firmware.' -ForegroundColor Green
            exit 0
        } catch {
            Write-Host "OTA update failed: $_" -ForegroundColor Yellow
            Write-Host 'Falling back to USB.' -ForegroundColor Yellow
        }
    } else {
        Write-Host 'Robot not found on the network, flashing over USB.' -ForegroundColor Yellow
    }
}

exit (Invoke-UsbFlash $Binary $Rest)
