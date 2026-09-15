//! Отдача телеметрии по UDP, обнаружение робота и приём команд движения.

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{ota, protocol, NetStatus, PORT_COMMAND, PORT_TELEMETRY};
use crate::params::ParamStore;
use crate::remote::RemoteState;
use crate::telemetry::{Telemetry, STATE_BALANCING};

const SUBSCRIPTION_TTL: Duration = Duration::from_secs(3);
pub const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn spawn(
    telemetry: Arc<Telemetry>,
    params: Arc<ParamStore>,
    status: Arc<NetStatus>,
    remote: Arc<RemoteState>,
) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("udp".into())
        .stack_size(6144)
        .spawn(move || {
            if let Err(e) = run(telemetry, params, status, remote) {
                log::error!("Телеметрия UDP: {e:#}");
            }
        })?;
    Ok(())
}

fn run(
    telemetry: Arc<Telemetry>,
    params: Arc<ParamStore>,
    status: Arc<NetStatus>,
    remote: Arc<RemoteState>,
) -> anyhow::Result<()> {
    while matches!(status.get(), super::LinkState::Down) {
        std::thread::sleep(Duration::from_millis(200));
    }

    let socket = UdpSocket::bind(("0.0.0.0", PORT_TELEMETRY))?;
    let started = Instant::now();
    let mut subscriber: Option<(SocketAddr, Instant)> = None;
    let mut buf = [0u8; 256];
    let mut seq = 0u32;
    let mut next_send = Instant::now();

    loop {
        let hz = params.get("telemetry_hz").unwrap_or(100.0).clamp(1.0, 200.0);
        let period = Duration::from_micros((1_000_000.0 / hz) as u64);

        if let Some((_, last_seen)) = subscriber {
            if last_seen.elapsed() >= SUBSCRIPTION_TTL {
                log::info!("Телеметрия: подписчик замолчал, останавливаю поток");
                subscriber = None;
            }
        }

        let now = Instant::now();
        if let (Some((addr, _)), true) = (subscriber, now >= next_send) {
            next_send += period;
            if next_send <= now {
                next_send = now + period;
            }

            let packet = encode(&telemetry, seq, started.elapsed());
            seq = seq.wrapping_add(1);

            if let Err(e) = socket.send_to(&packet, addr) {
                log::warn!("Телеметрия: отправка на {addr} не удалась: {e}");
                next_send = now + Duration::from_millis(100);
            }
        }

        let wait = if subscriber.is_some() {
            next_send.saturating_duration_since(Instant::now())
        } else {
            Duration::from_millis(200)
        };
        socket.set_read_timeout(Some(wait.clamp(
            Duration::from_millis(1),
            Duration::from_millis(200),
        )))?;

        let Ok((len, from)) = socket.recv_from(&mut buf) else {
            continue;
        };

        let Some(header) = protocol::parse_header(&buf[..len]) else {
            continue;
        };

        match header.kind {
            protocol::MSG_HELLO => {
                let reply = protocol::encode_info(
                    header.seq,
                    &crate::net::wifi::portal_ssid(),
                    FIRMWARE_VERSION,
                    PORT_COMMAND,
                );
                let _ = socket.send_to(&reply, from);

                if !matches!(subscriber, Some((addr, _)) if addr == from) {
                    log::info!("Телеметрия: подписчик {from}");
                    next_send = Instant::now();
                }
                subscriber = Some((from, Instant::now()));
            }
            protocol::MSG_DRIVE => {
                if let Some((_seq, forward, turn)) = protocol::parse_drive(&buf[..len]) {
                    // Передаем сырые значения, таймер фиксируется на кристалле аппаратно
                    remote.set_drive(forward, turn);
                }
            }
            _ => {}
        }
    }
}

fn encode(telemetry: &Telemetry, seq: u32, uptime: Duration) -> [u8; protocol::TELEMETRY_LEN] {
    let snapshot = telemetry.snapshot();

    let mut flags = 0u8;
    if snapshot.state == STATE_BALANCING {
        flags |= protocol::FLAG_ARMED;
    }
    if ota::in_progress() {
        flags |= protocol::FLAG_OTA;
    }
    if snapshot.imu_errors > 0 {
        flags |= protocol::FLAG_IMU_FAULT;
    }

    protocol::encode_telemetry(&snapshot, seq, uptime.as_millis() as u32, flags)
}