//! Подстройка параметров на ходу через UART0.
//!
//! Дублирует то, что умеет UI по сети, и остаётся единственным способом
//! что-то поправить, когда Wi-Fi недоступен — например, в первые секунды
//! после старта или если робот не нашёл ни одной знакомой сети.
//!
//! Команды (в мониторе espflash печатаешь строку и жмёшь Enter):
//!
//! ```text
//! ka 1500     жёсткость по углу, шаг/с² на градус
//! kd 280      демпфирование по гироскопу, шаг/с² на °/с
//! show        показать текущие значения
//! save        записать в NVS, чтобы пережили перезагрузку
//! reset       вернуть зашитые значения
//! cal         калибровка гироскопа на ходу (робот должен стоять)
//! ```
//!
//! Полный список ключей — в `params.rs`; здесь принимается любой из них.

use std::sync::Arc;

use esp_idf_svc::sys::{uart_driver_install, uart_read_bytes, ESP_OK};

use crate::params::{ParamStore, CATALOG};
use crate::storage::Storage;
use crate::request_gyro_calibration;
use crate::request_autotune;

const UART_NUM: u32 = 0;
const RX_BUFFER: i32 = 512;
/// Тик FreeRTOS здесь 1 кГц (см. `sdkconfig.defaults`), то есть 1 тик = 1 мс.
const READ_TIMEOUT_TICKS: u32 = 100;
const MAX_LINE: usize = 48;

struct Console {
    params: Arc<ParamStore>,
    storage: Arc<Storage>,
}

/// Запускает слушатель UART.
///
/// Ошибку установки драйвера фатальной не считаем: без тюнинга робот
/// просто работает на текущих значениях.
pub fn spawn(params: &Arc<ParamStore>, storage: &Arc<Storage>) {
    let err = unsafe { uart_driver_install(UART_NUM, RX_BUFFER, 0, 0, core::ptr::null_mut(), 0) };

    if err != ESP_OK {
        log::warn!("Тюнинг по UART недоступен (uart_driver_install = {err})");
        return;
    }

    let console = Console {
        params: params.clone(),
        storage: storage.clone(),
    };

    let spawned = std::thread::Builder::new()
        .name("tuning".into())
        .stack_size(4096)
        .spawn(move || listen(console));

    match spawned {
        Ok(_) => log::info!("Тюнинг по UART включён. Команды: <ключ> <значение> / show / save / reset"),
        Err(e) => log::warn!("Не удалось запустить тюнинг по UART: {e:?}"),
    }
}

fn listen(console: Console) {
    let mut buf = [0u8; 64];
    let mut line = String::with_capacity(MAX_LINE);

    loop {
        let n = unsafe {
            uart_read_bytes(
                UART_NUM,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                READ_TIMEOUT_TICKS,
            )
        };

        if n <= 0 {
            continue;
        }

        for &byte in &buf[..n as usize] {
            match byte {
                b'\n' | b'\r' => {
                    let cmd = line.trim();
                    if !cmd.is_empty() {
                        handle(&console, cmd);
                    }
                    line.clear();
                }
                0x20..=0x7E if line.len() < MAX_LINE => line.push(byte as char),
                _ => {}
            }
        }
    }
}

fn handle(console: &Console, line: &str) {
    let mut parts = line.split_whitespace();
    let Some(key) = parts.next() else {
        return;
    };
    let key = key.to_ascii_lowercase();

    match key.as_str() {
        "show" | "s" => {
            print_all(&console.params);
            return;
        }
        "save" => {
            match console.storage.save_params(&console.params) {
                Ok(()) => log::warn!("TUNE: параметры сохранены в NVS"),
                Err(e) => log::error!("TUNE: сохранить не удалось: {e:#}"),
            }
            return;
        }
        "reset" => {
            console.params.reset();
            log::warn!("TUNE: возвращены зашитые значения (не сохранены)");
            print_all(&console.params);
            return;
        }
        "calibrate" | "cal" => {
            // Запрос калибровки гироскопа (обработка в main loop)
            request_gyro_calibration();
            log::warn!("TUNE: запрос калибровки гироскопа отправлен");
            return;
        }
        "autotune" | "tune" => {
            // Запрос автонастройки PID
            request_autotune();
            log::warn!("TUNE: запущена автонастройка PID — ДЕРЖИ РОБОТА ВЕРТИКАЛЬНО!");
            return;
        }
        "help" | "h" | "?" => {
            print_help();
            return;
        }
        _ => {}
    }

    let Some(raw) = parts.next() else {
        log::warn!("TUNE: нужно значение, например `{key} 1500`");
        return;
    };

    match raw.parse::<f32>() {
        Ok(value) => match console.params.set(&key, value) {
            Some(accepted) if (accepted - value).abs() > f32::EPSILON => {
                log::warn!("TUNE: {key} = {accepted} (запрошенное {value} вне диапазона)")
            }
            Some(accepted) => log::warn!("TUNE: {key} = {accepted}"),
            None => {
                log::warn!("TUNE: неизвестный параметр `{key}`");
                print_help();
            }
        },
        Err(_) => log::warn!("TUNE: `{raw}` не число"),
    }
}

fn print_all(params: &ParamStore) {
    let values: Vec<String> = CATALOG
        .iter()
        .filter_map(|info| params.get(info.key).map(|v| format!("{}={v}", info.key)))
        .collect();
    log::warn!("TUNE {}", values.join(" "));
}

fn print_help() {
    for info in CATALOG {
        log::warn!(
            "TUNE {:<13} {:>10}..{:<10} [{}] — {}",
            info.key,
            info.min,
            info.max,
            info.unit,
            info.about
        );
    }
}
