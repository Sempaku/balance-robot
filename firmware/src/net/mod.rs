//! Сетевая часть: Wi-Fi, обнаружение робота, телеметрия, команды, OTA.
//!
//! Всё, что здесь, живёт на ядре 0 и не имеет права блокировать контур
//! управления на ядре 1. Поэтому обмена данными с контуром через мьютексы
//! нет — только атомики и снимки состояния.

pub mod command;
pub mod http_api;
pub mod ota;
pub mod portal;
pub mod protocol;
pub mod telemetry_tx;
pub mod wifi;

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;

/// Порт телеметрии и команд движения (UDP): теряем пакеты, но не задерживаем.
pub const PORT_TELEMETRY: u16 = 5555;
/// Порт настроек и OTA (TCP): здесь важна доставка, а не задержка.
pub const PORT_COMMAND: u16 = 5556;

const MODE_DOWN: u8 = 0;
const MODE_STATION: u8 = 1;
const MODE_PORTAL: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    Down,
    /// Робот в домашней сети, UI найдёт его через discovery.
    Station { ssid: String, ip: Ipv4Addr },
    /// Ни одна известная сеть не нашлась: робот раздаёт свою для настройки.
    Portal { ssid: String, ip: Ipv4Addr },
}

/// Состояние сети, доступное всем задачам без владения Wi-Fi драйвером.
pub struct NetStatus {
    mode: AtomicU8,
    ip: AtomicU32,
    ssid: Mutex<String>,
}

impl NetStatus {
    pub fn new() -> Self {
        Self {
            mode: AtomicU8::new(MODE_DOWN),
            ip: AtomicU32::new(0),
            ssid: Mutex::new(String::new()),
        }
    }

    pub fn set(&self, state: LinkState) {
        let (mode, ip, ssid) = match state {
            LinkState::Down => (MODE_DOWN, Ipv4Addr::UNSPECIFIED, String::new()),
            LinkState::Station { ssid, ip } => (MODE_STATION, ip, ssid),
            LinkState::Portal { ssid, ip } => (MODE_PORTAL, ip, ssid),
        };
        if let Ok(mut slot) = self.ssid.lock() {
            *slot = ssid;
        }
        self.ip.store(u32::from(ip), Ordering::Relaxed);
        self.mode.store(mode, Ordering::Release);
    }

    pub fn get(&self) -> LinkState {
        let ip = Ipv4Addr::from(self.ip.load(Ordering::Relaxed));
        let ssid = self
            .ssid
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| String::new());
        match self.mode.load(Ordering::Acquire) {
            MODE_STATION => LinkState::Station { ssid, ip },
            MODE_PORTAL => LinkState::Portal { ssid, ip },
            _ => LinkState::Down,
        }
    }

}

impl Default for NetStatus {
    fn default() -> Self {
        Self::new()
    }
}
