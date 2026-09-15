//! Бинарный формат UDP-пакетов. Должен совпадать байт в байт с C# стороной.
//!
//! Формат фиксированный, а не JSON: на 100 Гц разбор текста впустую тратил бы
//! и эфир, и время ядра 0.

use crate::telemetry::Snapshot;

/// «BLN1» в little-endian. Отсекает чужие пакеты, прилетевшие на тот же порт.
pub const MAGIC: u32 = 0x314E_4C42;
pub const VERSION: u8 = 1;

pub const MSG_TELEMETRY: u8 = 1;
pub const MSG_DRIVE: u8 = 2;
pub const MSG_HELLO: u8 = 3;
pub const MSG_INFO: u8 = 4;

pub const TELEMETRY_LEN: usize = 72;
pub const DRIVE_LEN: usize = 20;

pub const FLAG_ARMED: u8 = 1 << 0;
pub const FLAG_OTA: u8 = 1 << 1;
pub const FLAG_IMU_FAULT: u8 = 1 << 2;

/// Заголовок: magic, версия, тип, длина полезной части, порядковый номер.
pub fn write_header(out: &mut [u8], kind: u8, payload_len: u16, seq: u32) {
    out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    out[4] = VERSION;
    out[5] = kind;
    out[6..8].copy_from_slice(&payload_len.to_le_bytes());
    out[8..12].copy_from_slice(&seq.to_le_bytes());
}

pub struct Header {
    pub kind: u8,
    pub seq: u32,
}

/// Разбирает заголовок, отбрасывая всё, что не наш протокол.
pub fn parse_header(data: &[u8]) -> Option<Header> {
    if data.len() < 12 {
        return None;
    }
    let magic = u32::from_le_bytes(data[0..4].try_into().ok()?);
    if magic != MAGIC || data[4] != VERSION {
        return None;
    }
    Some(Header {
        kind: data[5],
        seq: u32::from_le_bytes(data[8..12].try_into().ok()?),
    })
}

/// Разбирает команду движения MSG_DRIVE (20 байт: 12 заголовок + 4 forward + 4 turn).
pub fn parse_drive(data: &[u8]) -> Option<(u32, f32, f32)> {
    if data.len() < DRIVE_LEN {
        return None;
    }
    let header = parse_header(data)?;
    if header.kind != MSG_DRIVE {
        return None;
    }
    let forward = f32::from_le_bytes(data[12..16].try_into().ok()?);
    let turn = f32::from_le_bytes(data[16..20].try_into().ok()?);
    if !forward.is_finite() || !turn.is_finite() {
        return None;
    }
    Some((header.seq, forward, turn))
}

pub fn encode_telemetry(
    snapshot: &Snapshot,
    seq: u32,
    uptime_ms: u32,
    flags: u8,
) -> [u8; TELEMETRY_LEN] {
    let mut out = [0u8; TELEMETRY_LEN];
    write_header(&mut out, MSG_TELEMETRY, (TELEMETRY_LEN - 12) as u16, seq);

    out[12..16].copy_from_slice(&uptime_ms.to_le_bytes());
    out[16] = snapshot.state;
    out[17] = flags;
    out[18..20].copy_from_slice(&snapshot.imu_errors.to_le_bytes());

    let floats = [
        snapshot.angle,
        snapshot.accel_angle,
        snapshot.gyro,
        snapshot.accel_mag,
        snapshot.gyro_bias,
        snapshot.trim,
        snapshot.cmd,
        snapshot.wheel_rate,
        snapshot.term_p,
        snapshot.term_lean,
        snapshot.term_d,
    ];
    for (i, value) in floats.iter().enumerate() {
        let at = 20 + i * 4;
        out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    out[64..68].copy_from_slice(&snapshot.steps.to_le_bytes());
    out[68..70].copy_from_slice(&snapshot.dt_us.to_le_bytes());
    out[70..72].copy_from_slice(&snapshot.loop_us.to_le_bytes());
    out
}

/// Ответ на широковещательный HELLO.
pub fn encode_info(seq: u32, name: &str, version: &str, command_port: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(64);
    payload.extend_from_slice(&command_port.to_le_bytes());
    put_str(&mut payload, name);
    put_str(&mut payload, version);

    let mut out = vec![0u8; 12];
    write_header(&mut out, MSG_INFO, payload.len() as u16, seq);
    out.extend_from_slice(&payload);
    out
}

fn put_str(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    let len = bytes.len().min(u8::MAX as usize);
    out.push(len as u8);
    out.extend_from_slice(&bytes[..len]);
}