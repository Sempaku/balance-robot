//! Портал настройки Wi-Fi: робот раздаёт свою сеть и показывает форму.
//!
//! Нужен ровно тогда, когда робота принесли в незнакомое место: компьютера с
//! кабелем рядом может не быть, зато телефон есть всегда.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use esp_idf_svc::http::server::{Configuration as HttpConfiguration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::Write;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};

use super::{LinkState, NetStatus};
use crate::net::wifi::{access_point_config, portal_ssid, PORTAL_PASSWORD};
use crate::storage::{Storage, WifiNetwork};

/// Сколько держать портал, если к нему никто не подключился. После этого
/// имеет смысл ещё раз поискать знакомые сети: вдруг роутер просто перезагружался.
const PORTAL_TIMEOUT: Duration = Duration::from_secs(300);
const BODY_LIMIT: usize = 512;

/// Поднимает точку доступа с формой и ждёт, пока сеть не сохранят.
///
/// Возвращает `Ok` и когда сеть сохранена, и по истечении таймаута — в обоих
/// случаях наверху нужно просто снова попробовать подключиться.
pub fn run(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    storage: &Arc<Storage>,
    status: &Arc<NetStatus>,
) -> anyhow::Result<()> {
    let ap_config = access_point_config()?;

    // Смешанный режим, а не чистый AP: клиентский интерфейс нужен, чтобы
    // показать в форме список сетей, которые робот реально видит отсюда.
    if wifi.is_started()? {
        wifi.stop()?;
    }
    wifi.set_configuration(&Configuration::Mixed(
        ClientConfiguration::default(),
        ap_config,
    ))?;
    wifi.start()?;

    let visible = scan_ssids(wifi);
    let ip = wifi.wifi().ap_netif().get_ip_info()?.ip;
    let ssid = portal_ssid();

    log::warn!("Портал: сеть «{ssid}», пароль «{PORTAL_PASSWORD}», откройте http://{ip}/");
    status.set(LinkState::Portal {
        ssid: ssid.clone(),
        ip,
    });

    let saved = Arc::new(AtomicBool::new(false));
    let _server = serve(storage.clone(), saved.clone(), visible, ssid)?;

    let started = Instant::now();
    while !saved.load(Ordering::Acquire) && started.elapsed() < PORTAL_TIMEOUT {
        std::thread::sleep(Duration::from_millis(200));
    }

    if saved.load(Ordering::Acquire) {
        log::info!("Портал: сеть сохранена, подключаюсь");
    } else {
        log::info!("Портал: никто не подключился, пробую известные сети ещё раз");
    }

    // Сервер и точка доступа должны исчезнуть до попытки стать станцией,
    // иначе радио останется поделено между AP и STA.
    drop(_server);
    wifi.stop()?;
    Ok(())
}

fn scan_ssids(wifi: &mut BlockingWifi<EspWifi<'static>>) -> Vec<String> {
    let Ok(found) = wifi.scan() else {
        return Vec::new();
    };
    let mut names: Vec<String> = found
        .into_iter()
        .filter(|ap| !ap.ssid.is_empty())
        .map(|ap| ap.ssid.as_str().to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn serve(
    storage: Arc<Storage>,
    saved: Arc<AtomicBool>,
    visible: Vec<String>,
    ap_ssid: String,
) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&HttpConfiguration::default())?;

    let page = Arc::new(render_page(&visible, &ap_ssid));
    let form_page = page.clone();
    server.fn_handler("/", Method::Get, move |request| {
        let mut response = request.into_response(200, None, &HTML_HEADERS)?;
        response.write_all(form_page.as_bytes())?;
        anyhow::Ok(())
    })?;

    // Мьютекс, а не просто Arc: обработчики обязаны быть Fn, а запись в NVS
    // требует исключительного доступа.
    let storage = Mutex::new(storage);
    server.fn_handler("/save", Method::Post, move |mut request| {
        let mut buf = [0u8; BODY_LIMIT];
        let read = request.read(&mut buf)?;
        let body = core::str::from_utf8(&buf[..read]).unwrap_or("");

        let result = match parse_network(body) {
            Some(network) => {
                let storage = storage
                    .lock()
                    .map_err(|_| anyhow::anyhow!("хранилище занято"))?;
                match store_network(&storage, network) {
                    Ok(ssid) => {
                        saved.store(true, Ordering::Release);
                        Ok(ssid)
                    }
                    Err(e) => Err(format!("не удалось сохранить: {e}")),
                }
            }
            None => Err("имя сети не заполнено".to_string()),
        };

        let (code, text) = match result {
            Ok(ssid) => (
                200,
                format!(
                    "<h2>Сеть «{}» сохранена</h2><p>Робот подключается. \
                     Эта точка доступа сейчас исчезнет.</p>",
                    escape(&ssid)
                ),
            ),
            Err(message) => (
                400,
                format!("<h2>Ошибка</h2><p>{}</p><p><a href=\"/\">Назад</a></p>", escape(&message)),
            ),
        };

        let mut response = request.into_response(code, None, &HTML_HEADERS)?;
        response.write_all(wrap_html(&text).as_bytes())?;
        anyhow::Ok(())
    })?;

    Ok(server)
}

const HTML_HEADERS: [(&str, &str); 1] = [("Content-Type", "text/html; charset=utf-8")];

/// Добавляет сеть в список, заменяя запись с тем же именем.
fn store_network(storage: &Storage, network: WifiNetwork) -> anyhow::Result<String> {
    let mut networks = storage.load_networks();
    networks.retain(|n| n.ssid != network.ssid);
    let ssid = network.ssid.clone();
    // Введённая вручную сеть — самая свежая информация о месте, где робот
    // находится сейчас, поэтому пробовать её нужно первой.
    let top = networks.iter().map(|n| n.priority).max().unwrap_or(0);
    networks.push(WifiNetwork {
        priority: top + 1,
        ..network
    });
    storage.save_networks(&networks)?;
    Ok(ssid)
}

fn parse_network(body: &str) -> Option<WifiNetwork> {
    let mut ssid = String::new();
    let mut psk = String::new();

    for pair in body.split('&') {
        // Пара без '=' — мусор в теле запроса: пропускаем её, а не весь разбор.
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "ssid" => ssid = url_decode(value),
            "psk" => psk = url_decode(value),
            _ => {}
        }
    }

    if ssid.is_empty() {
        return None;
    }
    Some(WifiNetwork {
        ssid,
        psk,
        priority: 0,
    })
}

fn url_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = core::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn render_page(visible: &[String], ap_ssid: &str) -> String {
    let options = visible
        .iter()
        .map(|ssid| {
            let safe = escape(ssid);
            format!("<option value=\"{safe}\">{safe}</option>")
        })
        .collect::<Vec<_>>()
        .join("");

    let picker = if visible.is_empty() {
        String::new()
    } else {
        format!(
            "<label>Найденные сети\
             <select onchange=\"document.getElementById('ssid').value=this.value\">\
             <option value=\"\">— выбрать —</option>{options}</select></label>"
        )
    };

    wrap_html(&format!(
        "<h2>{}</h2>\
         <form method=\"post\" action=\"/save\">\
         {picker}\
         <label>Имя сети<input id=\"ssid\" name=\"ssid\" maxlength=\"32\" required></label>\
         <label>Пароль<input name=\"psk\" type=\"password\" maxlength=\"64\"></label>\
         <button type=\"submit\">Сохранить и подключиться</button>\
         </form>\
         <p class=\"hint\">Сеть запоминается навсегда, список хранится в самом роботе.</p>",
        escape(ap_ssid)
    ))
}

fn wrap_html(body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>Настройка робота</title><style>\
         body{{font-family:system-ui,sans-serif;margin:0;padding:24px;background:#12141a;color:#e8eaf0}}\
         form{{display:flex;flex-direction:column;gap:14px;max-width:360px}}\
         label{{display:flex;flex-direction:column;gap:6px;font-size:14px}}\
         input,select{{padding:10px;border-radius:8px;border:1px solid #333a4a;background:#1b1f28;color:inherit;font-size:16px}}\
         button{{padding:12px;border:0;border-radius:8px;background:#3d7eff;color:#fff;font-size:16px}}\
         .hint{{color:#8b93a7;font-size:13px;max-width:360px}}\
         </style></head><body>{body}</body></html>"
    )
}
