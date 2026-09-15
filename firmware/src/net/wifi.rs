//! Подключение к Wi-Fi со списком сетей и порталом как запасным вариантом.
//!
//! Схема из трёх уровней, чтобы робот оставался доступен где угодно:
//!   1. сети из NVS (правятся по сети или через портал);
//!   2. сети, вшитые при сборке (`wifi_seed.json`) — для чистой платы;
//!   3. если ничего не нашлось — робот сам раздаёт точку доступа с формой.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};

use super::{LinkState, NetStatus};
use crate::net::portal;
use crate::storage::{seed_networks, Storage, WifiNetwork};

/// Сколько ждать между проверками живости соединения.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);
/// Сколько подряд неудачных попыток вернуть станцию, прежде чем поднять портал.
const RECONNECT_ATTEMPTS: u32 = 5;
/// Пароль точки доступа робота. Открытую сеть делать нельзя: на неё
/// может зайти кто угодно и отправить команду движения.
pub const PORTAL_PASSWORD: &str = "balancebot";

/// Поднимает сеть и дальше сам следит за ней в фоне.
///
/// Возвращается сразу после первой попытки подключения, чтобы балансировка
/// стартовала, даже если Wi-Fi в этом месте недоступен.
pub fn spawn(
    modem: Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs_part: EspDefaultNvsPartition,
    storage: Arc<Storage>,
    status: Arc<NetStatus>,
) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("wifi".into())
        .stack_size(8192)
        .spawn(move || {
            if let Err(e) = run(modem, sysloop, nvs_part, storage, status) {
                log::error!("Wi-Fi: задача завершилась с ошибкой: {e:#}");
            }
        })?;
    Ok(())
}

fn run(
    modem: Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs_part: EspDefaultNvsPartition,
    storage: Arc<Storage>,
    status: Arc<NetStatus>,
) -> anyhow::Result<()> {
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sysloop.clone(), Some(nvs_part))?,
        sysloop,
    )?;

    let mut failures = 0u32;

    loop {
        match connect_known(&mut wifi, &storage) {
            Ok(state) => {
                log::info!("Wi-Fi: {}", describe(&state));
                status.set(state);
                failures = 0;
                watch(&mut wifi, &status);
                log::warn!("Wi-Fi: соединение потеряно, переподключаюсь");
                status.set(LinkState::Down);
            }
            Err(e) => {
                failures += 1;
                log::warn!("Wi-Fi: подключиться не удалось ({e:#}), попытка {failures}");
                status.set(LinkState::Down);
            }
        }

        if failures >= RECONNECT_ATTEMPTS {
            // Дальше пытаться бессмысленно: либо мы в новом месте, либо пароль
            // сменился. Отдаём управление человеку через форму на портале.
            log::warn!("Wi-Fi: известных сетей нет, поднимаю портал настройки");
            match portal::run(&mut wifi, &storage, &status) {
                // Портал сохранил новую сеть — пробуем её без перезагрузки.
                Ok(()) => failures = 0,
                Err(e) => log::error!("Портал: {e:#}"),
            }
            status.set(LinkState::Down);
        }
    }
}

fn describe(state: &LinkState) -> String {
    match state {
        LinkState::Station { ssid, ip } => format!("подключён к «{ssid}», адрес {ip}"),
        LinkState::Portal { ssid, ip } => format!("портал «{ssid}» на {ip}"),
        LinkState::Down => "нет связи".into(),
    }
}

/// Перебирает известные сети, начиная с самой приоритетной из видимых в эфире.
fn connect_known(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    storage: &Storage,
) -> anyhow::Result<LinkState> {
    let known = known_networks(storage);
    if known.is_empty() {
        anyhow::bail!("список сетей пуст");
    }

    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;
    if !wifi.is_started()? {
        wifi.start()?;
    }

    // Сканирование нужно не столько для выбора, сколько чтобы не тратить по
    // 10 секунд таймаута на сети, которых здесь заведомо нет.
    let visible = wifi.scan().unwrap_or_else(|e| {
        log::warn!("Wi-Fi: скан не удался ({e}), пробую вслепую");
        Vec::new()
    });

    let mut candidates: Vec<(&WifiNetwork, i8)> = if visible.is_empty() {
        known.iter().map(|n| (n, i8::MIN)).collect()
    } else {
        known
            .iter()
            .filter_map(|n| {
                visible
                    .iter()
                    .filter(|ap| ap.ssid.as_str() == n.ssid)
                    .map(|ap| ap.signal_strength)
                    .max()
                    .map(|rssi| (n, rssi))
            })
            .collect()
    };

    if candidates.is_empty() {
        anyhow::bail!("ни одна из {} известных сетей не видна", known.len());
    }

    // Сначала приоритет, заданный человеком, при равенстве — уровень сигнала.
    candidates.sort_by(|a, b| {
        b.0.priority
            .cmp(&a.0.priority)
            .then(b.1.cmp(&a.1))
    });

    let mut last_error = None;
    for (network, rssi) in candidates {
        log::info!("Wi-Fi: пробую «{}» (RSSI {rssi})", network.ssid);
        match join(wifi, network) {
            Ok(ip) => {
                return Ok(LinkState::Station {
                    ssid: network.ssid.clone(),
                    ip,
                })
            }
            Err(e) => {
                log::warn!("Wi-Fi: «{}» не подошла: {e}", network.ssid);
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("подключиться не удалось")))
}

fn join(wifi: &mut BlockingWifi<EspWifi<'static>>, network: &WifiNetwork) -> anyhow::Result<Ipv4Addr> {
    let auth_method = if network.psk.is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    };

    let config = ClientConfiguration {
        ssid: network
            .ssid
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("SSID длиннее 32 символов"))?,
        password: network
            .psk
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("пароль длиннее 64 символов"))?,
        auth_method,
        ..Default::default()
    };

    wifi.set_configuration(&Configuration::Client(config))?;
    wifi.connect()?;
    wifi.wait_netif_up()?;

    let ip = wifi.wifi().sta_netif().get_ip_info()?.ip;
    Ok(ip)
}

/// Сети из NVS плюс вшитые при сборке. При совпадении SSID выигрывает NVS:
/// то, что человек ввёл последним, актуальнее зашитого полгода назад.
fn known_networks(storage: &Storage) -> Vec<WifiNetwork> {
    let mut networks = storage.load_networks();
    for seed in seed_networks() {
        if !networks.iter().any(|n| n.ssid == seed.ssid) {
            networks.push(seed);
        }
    }
    networks
}

/// Блокируется, пока соединение живо.
fn watch(wifi: &mut BlockingWifi<EspWifi<'static>>, status: &NetStatus) {
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        let alive = wifi.is_connected().unwrap_or(false) && wifi.is_up().unwrap_or(false);
        if !alive {
            return;
        }
        // Переезд на другой IP после смены аренды DHCP ломает UI молча,
        // поэтому адрес перечитываем, а не запоминаем один раз.
        if let (LinkState::Station { ssid, ip }, Ok(info)) =
            (status.get(), wifi.wifi().sta_netif().get_ip_info())
        {
            if info.ip != ip {
                log::info!("Wi-Fi: адрес сменился на {}", info.ip);
                status.set(LinkState::Station { ssid, ip: info.ip });
            }
        }
    }
}

/// Имя точки доступа робота. Хвост из MAC отличает робота от соседних
/// устройств, если их в комнате несколько.
pub fn portal_ssid() -> String {
    let mut mac = [0u8; 6];
    // SAFETY: функция только заполняет переданный буфер длиной 6 байт.
    let ok = unsafe {
        esp_idf_svc::sys::esp_read_mac(
            mac.as_mut_ptr(),
            esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_WIFI_SOFTAP,
        ) == esp_idf_svc::sys::ESP_OK
    };
    if ok {
        format!("BalanceBot-{:02X}{:02X}", mac[4], mac[5])
    } else {
        "BalanceBot".to_string()
    }
}

pub fn access_point_config() -> anyhow::Result<AccessPointConfiguration> {
    Ok(AccessPointConfiguration {
        ssid: portal_ssid()
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("имя точки доступа слишком длинное"))?,
        password: PORTAL_PASSWORD
            .try_into()
            .map_err(|_| anyhow::anyhow!("пароль точки доступа слишком длинный"))?,
        auth_method: AuthMethod::WPA2Personal,
        channel: 1,
        max_connections: 2,
        ..Default::default()
    })
}
