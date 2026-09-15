//! Команды по TCP: настройки, список сетей, OTA, чёрный ящик.

use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::telemetry_tx::FIRMWARE_VERSION;
use super::{ota, LinkState, NetStatus, PORT_COMMAND};
use crate::params::ParamStore;
use crate::remote::RemoteState;
use crate::storage::{Storage, WifiNetwork};
use crate::telemetry::Telemetry;

const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const LINE_LIMIT: usize = 2048;
const MAX_NETWORKS: usize = 5;

pub struct Services {
    pub params: Arc<ParamStore>,
    pub storage: Arc<Storage>,
    pub status: Arc<NetStatus>,
    pub remote: Arc<RemoteState>,
    pub telemetry: Arc<Telemetry>,
}

pub fn spawn(services: Services) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("tcp-cmd".into())
        .stack_size(12288)
        .spawn(move || {
            if let Err(e) = run(services) {
                log::error!("Команды TCP: {e:#}");
            }
        })?;
    Ok(())
}

fn run(services: Services) -> anyhow::Result<()> {
    while matches!(services.status.get(), LinkState::Down) {
        std::thread::sleep(Duration::from_millis(200));
    }

    let listener = TcpListener::bind(("0.0.0.0", PORT_COMMAND))?;
    log::info!("Команды: слушаю TCP {PORT_COMMAND}");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let peer = stream.peer_addr().ok();
                log::info!("Команды: подключился {peer:?}");
                if let Err(e) = serve(stream, &services) {
                    log::warn!("Команды: сессия {peer:?} закрыта: {e:#}");
                }
            }
            Err(e) => log::warn!("Команды: приём соединения не удался: {e}"),
        }
    }

    Ok(())
}

fn serve(stream: TcpStream, services: &Services) -> anyhow::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_nodelay(true)?;

    let mut writer = &stream;
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();

    loop {
        line.clear();
        let read = read_line_limited(&mut reader, &mut line)?;
        if read == 0 {
            return Ok(());
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(e) => {
                respond(&mut writer, &json!({"ok": false, "error": format!("не JSON: {e}")}))?;
                continue;
            }
        };

        let command = request.get("cmd").and_then(Value::as_str).unwrap_or("");

        if command == "ota" {
            return handle_ota(&request, &mut reader, &mut writer);
        }

        let response = match dispatch(command, &request, services) {
            Ok(mut value) => {
                value["ok"] = Value::Bool(true);
                value
            }
            Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
        };
        respond(&mut writer, &response)?;

        if command == "reboot" {
            ota::reboot_soon(Duration::from_millis(200));
        }
    }
}

fn dispatch(command: &str, request: &Value, services: &Services) -> anyhow::Result<Value> {
    match command {
        "info" => Ok(info(services)),
        "param_catalog" => Ok(json!({ "params": ParamStore::catalog_json() })),
        "get_params" => Ok(json!({ "params": services.params.to_json() })),
        "set_params" => set_params(request, services),
        "save_params" => {
            services.storage.save_params(&services.params)?;
            Ok(json!({}))
        }
        "reset_params" => {
            services.params.reset();
            services.storage.clear_params()?;
            Ok(json!({ "params": services.params.to_json() }))
        }
        "get_wifi" => Ok(json!({ "networks": networks_json(services) })),
        "set_wifi" => set_wifi(request, services),
        "del_wifi" => del_wifi(request, services),
        "arm" => {
            services.remote.request_arm();
            Ok(json!({}))
        }
        "disarm" => {
            services.remote.request_disarm();
            Ok(json!({}))
        }
        "get_blackbox" => Ok(json!({ "frames": services.telemetry.get_blackbox_json() })),
        "clear_blackbox" => {
            services.telemetry.reset_blackbox();
            Ok(json!({}))
        }
        "reboot" => Ok(json!({})),
        other => anyhow::bail!("неизвестная команда `{other}`"),
    }
}

fn info(services: &Services) -> Value {
    let (mode, ssid, ip) = match services.status.get() {
        LinkState::Station { ssid, ip } => ("station", ssid, ip.to_string()),
        LinkState::Portal { ssid, ip } => ("portal", ssid, ip.to_string()),
        LinkState::Down => ("down", String::new(), String::new()),
    };

    json!({
        "name": crate::net::wifi::portal_ssid(),
        "firmware": FIRMWARE_VERSION,
        "slot": ota::running_slot_label(),
        "mode": mode,
        "ssid": ssid,
        "ip": ip,
    })
}

fn set_params(request: &Value, services: &Services) -> anyhow::Result<Value> {
    let Some(object) = request.as_object() else {
        anyhow::bail!("ожидался объект");
    };

    let mut applied = serde_json::Map::new();
    let mut unknown = Vec::new();

    for (key, value) in object {
        if key == "cmd" {
            continue;
        }
        let Some(number) = value.as_f64() else {
            unknown.push(key.clone());
            continue;
        };
        match services.params.set(key, number as f32) {
            Some(accepted) => {
                applied.insert(key.clone(), json!(accepted));
            }
            None => unknown.push(key.clone()),
        }
    }

    if applied.is_empty() && !unknown.is_empty() {
        anyhow::bail!("не распознан ни один параметр: {}", unknown.join(", "));
    }

    Ok(json!({ "applied": applied, "ignored": unknown }))
}

fn networks_json(services: &Services) -> Value {
    let current_ssid = match services.status.get() {
        LinkState::Station { ssid, .. } => ssid,
        _ => String::new(),
    };

    let networks: Vec<Value> = services
        .storage
        .all_known_networks()
        .into_iter()
        .map(|n| {
            let is_active = !current_ssid.is_empty() && n.ssid == current_ssid;
            json!({
                "ssid": n.ssid,
                "priority": n.priority,
                "active": is_active
            })
        })
        .collect();

    Value::Array(networks)
}

fn set_wifi(request: &Value, services: &Services) -> anyhow::Result<Value> {
    let ssid = request
        .get("ssid")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("не указано имя сети"))?;
    let psk = request.get("psk").and_then(Value::as_str).unwrap_or("");
    let priority = request
        .get("priority")
        .and_then(Value::as_i64)
        .unwrap_or(0) as i32;

    let mut networks = services.storage.load_networks();
    networks.retain(|n| n.ssid != ssid);
    networks.push(WifiNetwork {
        ssid: ssid.to_string(),
        psk: psk.to_string(),
        priority,
    });

    if networks.len() > MAX_NETWORKS {
        networks.sort_by(|a, b| b.priority.cmp(&a.priority));
        networks.truncate(MAX_NETWORKS);
    }

    services.storage.save_networks(&networks)?;
    Ok(json!({ "networks": networks_json(services) }))
}

fn del_wifi(request: &Value, services: &Services) -> anyhow::Result<Value> {
    let ssid = request
        .get("ssid")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("не указано имя сети"))?;

    let mut networks = services.storage.load_networks();
    let before = networks.len();
    networks.retain(|n| n.ssid != ssid);
    if networks.len() == before {
        anyhow::bail!("сети «{ssid}» нет в списке");
    }

    services.storage.save_networks(&networks)?;
    Ok(json!({ "networks": networks_json(services) }))
}

fn handle_ota(
    request: &Value,
    reader: &mut BufReader<&TcpStream>,
    writer: &mut &TcpStream,
) -> anyhow::Result<()> {
    let size = request
        .get("size")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("не указан размер образа"))?;

    respond(writer, &json!({"ok": true, "ready": true, "size": size}))?;

    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(30)))?;

    match ota::receive(reader, size) {
        Ok(()) => {
            respond(writer, &json!({"ok": true, "written": size, "rebooting": true}))?;
            let _ = writer.flush();
            ota::reboot_soon(Duration::from_millis(300));
        }
        Err(e) => {
            respond(writer, &json!({"ok": false, "error": format!("{e:#}")}))?;
            Err(e)
        }
    }
}

fn respond(writer: &mut &TcpStream, value: &Value) -> anyhow::Result<()> {
    let mut text = serde_json::to_string(value)?;
    text.push('\n');
    writer.write_all(text.as_bytes())?;
    writer.flush()?;
    Ok(())
}

fn read_line_limited(
    reader: &mut BufReader<&TcpStream>,
    out: &mut String,
) -> anyhow::Result<usize> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = std::io::Read::read(reader, &mut byte)?;
        if read == 0 || byte[0] == b'\n' {
            out.push_str(&String::from_utf8_lossy(&bytes));
            return Ok(bytes.len() + read);
        }
        if bytes.len() >= LINE_LIMIT {
            anyhow::bail!("команда длиннее {LINE_LIMIT} байт");
        }
        bytes.push(byte[0]);
    }
}