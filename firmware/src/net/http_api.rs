//! Простой HTTP API для телеметрии и тюнинга.
//!
//! Endpoints:
//! - GET /telemetry — текущее состояние робота
//! - GET /params — все параметры
//! - GET / — минимальный HTML дашборд

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::NetStatus;
use crate::params::{ParamStore, CATALOG};
use crate::telemetry::Telemetry;

pub const HTTP_PORT: u16 = 80;
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub fn spawn(
    params: Arc<ParamStore>,
    telemetry: Arc<Telemetry>,
    status: Arc<NetStatus>,
) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("http-api".into())
        .stack_size(16384)
        .spawn(move || {
            if let Err(e) = run(params, telemetry, status) {
                log::error!("HTTP API: {e:#}");
            }
        })?;
    log::info!("HTTP API: запущен на порту {HTTP_PORT}");
    Ok(())
}

fn run(params: Arc<ParamStore>, telemetry: Arc<Telemetry>, status: Arc<NetStatus>) -> anyhow::Result<()> {
    while matches!(status.get(), super::LinkState::Down) {
        std::thread::sleep(Duration::from_millis(200));
    }

    let listener = TcpListener::bind(("0.0.0.0", HTTP_PORT))?;
    log::info!("HTTP API: слушаю порт {HTTP_PORT}");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if let Err(e) = handle_connection(stream, &params, &telemetry) {
                    log::warn!("HTTP сессия закрыта: {e}");
                }
            }
            Err(e) => log::warn!("HTTP: ошибка соединения: {e}"),
        }
    }

    Ok(())
}

fn handle_connection(
    stream: TcpStream,
    params: &Arc<ParamStore>,
    telemetry: &Arc<Telemetry>,
) -> anyhow::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();

    reader.read_line(&mut line)?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let method = parts[0];
    let path = parts[1];

    if method != "GET" {
        send_response(&stream, 405, "Method Not Allowed", "text/plain", b"Only GET supported")?;
        return Ok(());
    }

    match path {
        "/telemetry" => {
            let snapshot = telemetry.snapshot();
            let body = json!({
                "angle": snapshot.angle,
                "gyro": snapshot.gyro,
                "accel_angle": snapshot.accel_angle,
                "wheel_rate": snapshot.wheel_rate,
                "cmd": snapshot.cmd,
                "term_p": snapshot.term_p,
                "term_lean": snapshot.term_lean,
                "term_d": snapshot.term_d,
                "state": if snapshot.state == 1 { "BALANCING" } else { "IDLE" },
                "imu_errors": snapshot.imu_errors,
                "dt_us": snapshot.dt_us,
            });
            let body_bytes = body.to_string();
            send_response(&stream, 200, "OK", "application/json", body_bytes.as_bytes())?;
        }
        "/params" => {
            let body = params.to_json().to_string();
            send_response(&stream, 200, "OK", "application/json", body.as_bytes())?;
        }
        "/catalog" => {
            let body = ParamStore::catalog_json().to_string();
            send_response(&stream, 200, "OK", "application/json", body.as_bytes())?;
        }
        "/" => {
            let html = build_dashboard();
            send_response(&stream, 200, "OK", "text/html; charset=utf-8", html.as_bytes())?;
        }
        _ => {
            send_response(&stream, 404, "Not Found", "text/plain", b"Endpoint not found")?;
        }
    }

    Ok(())
}

fn send_response(
    stream: &TcpStream,
    status: u16,
    status_text: &str,
    content_type: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let mut writer = stream;
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status, status_text, content_type, body.len()
    );
    writer.write_all(response.as_bytes())?;
    writer.write_all(body)?;
    writer.flush()?;
    Ok(())
}

fn build_dashboard() -> String {
    r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>BalanceBot Dashboard</title>
    <style>
        * { box-sizing: border-box; }
        body { 
            font-family: 'Segoe UI', monospace, sans-serif; 
            background: #1a1a2e; 
            color: #eee; 
            padding: 20px; 
            margin: 0;
        }
        h1 { color: #0f0; margin-bottom: 20px; }
        .metrics { display: flex; flex-wrap: wrap; gap: 15px; margin-bottom: 30px; }
        .metric { 
            background: #16213e; 
            border: 1px solid #0f0;
            border-radius: 8px;
            padding: 20px; 
            min-width: 140px;
            text-align: center;
        }
        .value { font-size: 28px; font-weight: bold; color: #0f0; }
        .label { color: #888; font-size: 13px; margin-top: 5px; }
        .state-balancing { color: #0f0; }
        .state-idle { color: #888; }
        #log { 
            background: #0d1117; 
            border: 1px solid #333; 
            border-radius: 5px;
            padding: 15px; 
            font-family: monospace; 
            font-size: 12px;
            max-height: 300px; 
            overflow: auto;
            white-space: pre;
        }
        .error { color: #f55; }
    </style>
</head>
<body>
    <h1>🤖 BalanceBot Dashboard</h1>
    <div class="metrics">
        <div class="metric">
            <div class="label">Угол (°)</div>
            <div class="value" id="angle">--</div>
        </div>
        <div class="metric">
            <div class="label">Гироскоп (°/с)</div>
            <div class="value" id="gyro">--</div>
        </div>
        <div class="metric">
            <div class="label">Accel Angle (°)</div>
            <div class="value" id="accel">--</div>
        </div>
        <div class="metric">
            <div class="label">Скорость (шаг/с)</div>
            <div class="value" id="rate">--</div>
        </div>
        <div class="metric">
            <div class="label">Команда</div>
            <div class="value" id="cmd">--</div>
        </div>
        <div class="metric">
            <div class="label">P-терм</div>
            <div class="value" id="p_term">--</div>
        </div>
        <div class="metric">
            <div class="label">D-терм</div>
            <div class="value" id="d_term">--</div>
        </div>
        <div class="metric">
            <div class="label">Состояние</div>
            <div class="value" id="state">--</div>
        </div>
        <div class="metric">
            <div class="label">Ошибки IMU</div>
            <div class="value" id="imu_err">--</div>
        </div>
    </div>
    <h2>Raw Data</h2>
    <div id="log">Подключение...</div>
    <script>
        const els = {
            angle: document.getElementById('angle'),
            gyro: document.getElementById('gyro'),
            accel: document.getElementById('accel'),
            rate: document.getElementById('rate'),
            cmd: document.getElementById('cmd'),
            p_term: document.getElementById('p_term'),
            d_term: document.getElementById('d_term'),
            state: document.getElementById('state'),
            imu_err: document.getElementById('imu_err'),
            log: document.getElementById('log')
        };

        async function fetchTelemetry() {
            try {
                const r = await fetch('/telemetry');
                if (!r.ok) throw new Error('HTTP ' + r.status);
                const d = await r.json();
                
                els.angle.textContent = d.angle.toFixed(2);
                els.gyro.textContent = d.gyro.toFixed(1);
                els.accel.textContent = d.accel_angle.toFixed(2);
                els.rate.textContent = d.wheel_rate.toFixed(0);
                els.cmd.textContent = d.cmd.toFixed(0);
                els.p_term.textContent = d.term_p.toFixed(0);
                els.d_term.textContent = d.term_d.toFixed(0);
                els.state.textContent = d.state;
                els.state.className = 'value ' + (d.state === 'BALANCING' ? 'state-balancing' : 'state-idle');
                els.imu_err.textContent = d.imu_errors;
                els.log.textContent = JSON.stringify(d, null, 2);
                els.log.classList.remove('error');
            } catch(e) {
                els.log.textContent = 'Ошибка: ' + e.message;
                els.log.classList.add('error');
            }
        }
        setInterval(fetchTelemetry, 200);
        fetchTelemetry();
    </script>
</body>
</html>"#.to_string()
}
