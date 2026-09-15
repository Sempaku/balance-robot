# Монитор UART без рукопожатия с чипом.
#
# `espflash monitor` перед чтением пытается связаться с загрузчиком по DTR/RTS,
# а на этой плате линия автосброса не работает — получается «Failed to connect»
# вместо лога. Здесь порт просто открывается на чтение.
#
# Выход — Ctrl+C. Порт можно задать аргументом или через $env:ESPFLASH_PORT.

param(
    [string]$Port,
    [int]$Baud = 115200
)

$ErrorActionPreference = 'Stop'

if (-not $Port) { $Port = $env:ESPFLASH_PORT }
if (-not $Port) {
    $live = [System.IO.Ports.SerialPort]::GetPortNames()
    $Port = Get-PnpDevice -Class Ports -ErrorAction SilentlyContinue |
        Where-Object { $_.FriendlyName -match 'CH340|CP210|FTDI|USB-SERIAL|UART' } |
        ForEach-Object { if ($_.FriendlyName -match '\((COM\d+)\)') { $Matches[1] } } |
        Where-Object { $live -contains $_ } |
        Select-Object -First 1
}
if (-not $Port) { throw 'No live USB-serial port found' }

$serial = New-Object System.IO.Ports.SerialPort($Port, $Baud, 'None', 8, 'One')
# DTR и RTS на плате разведены на EN и GPIO0. Поднимать их нельзя: чип уйдёт
# в сброс или в режим загрузки ровно в момент открытия монитора.
$serial.DtrEnable = $false
$serial.RtsEnable = $false
$serial.ReadTimeout = 200
$serial.Encoding = [System.Text.Encoding]::UTF8

# Прошивка логирует по-русски, поэтому консоль тоже переводим в UTF-8.
$previousEncoding = [Console]::OutputEncoding
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$serial.Open()
Write-Host "Monitor on $Port at $Baud. Press EN to restart the board, Q to exit." -ForegroundColor Cyan
Write-Host ''

try {
    while ($true) {
        # Ctrl+C доходит не во всяком терминале, поэтому предусмотрен и выход
        # по клавише. KeyAvailable недоступен при перенаправленном вводе.
        try {
            if ([Console]::KeyAvailable) {
                $key = [Console]::ReadKey($true)
                if ($key.Key -eq 'Q' -or $key.Key -eq 'Escape') { break }
            }
        } catch { }

        $chunk = $serial.ReadExisting()
        if ($chunk) {
            [Console]::Out.Write($chunk)
            # Без явного сброса лог виден только после выхода из монитора,
            # когда вывод перенаправлен (например, при запуске из cargo run).
            [Console]::Out.Flush()
        } else {
            Start-Sleep -Milliseconds 30
        }
    }
} finally {
    $serial.Close()
    [Console]::OutputEncoding = $previousEncoding
}
