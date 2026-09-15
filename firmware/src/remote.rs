//! Заявки от сети к контуру управления.
//!
//! Контур на ядре 1 не имеет права ждать мьютекс, который держит сетевая
//! задача, поэтому сеть не вызывает контроллер напрямую, а поднимает флаг.
//! Контур забирает его в своём темпе и сам решает, можно ли выполнить.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

/// Таймаут watchdog для команд движения (300 мс).
pub const DRIVE_WATCHDOG_MS: u32 = 300;

pub struct RemoteState {
    arm: AtomicBool,
    disarm: AtomicBool,
    // f32 в диапазоне -1.0..1.0, масштабированный в i32 (× 10 000)
    drive_forward: AtomicI32,
    drive_turn: AtomicI32,
    last_drive_ms: AtomicU32,
}

impl Default for RemoteState {
    fn default() -> Self {
        Self {
            arm: AtomicBool::new(false),
            disarm: AtomicBool::new(false),
            drive_forward: AtomicI32::new(0),
            drive_turn: AtomicI32::new(0),
            last_drive_ms: AtomicU32::new(0),
        }
    }
}

impl RemoteState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request_arm(&self) {
        self.arm.store(true, Ordering::Release);
    }

    pub fn request_disarm(&self) {
        self.arm.store(false, Ordering::Release);
        self.disarm.store(true, Ordering::Release);
    }

    pub fn take_arm(&self) -> bool {
        self.arm.swap(false, Ordering::AcqRel)
    }

    pub fn take_disarm(&self) -> bool {
        self.disarm.swap(false, Ordering::AcqRel)
    }

    /// Запись команды движения (вызывается из UDP-потока на ядре 0).
    pub fn set_drive(&self, forward: f32, turn: f32) {
        let f = (forward.clamp(-1.0, 1.0) * 10_000.0) as i32;
        let t = (turn.clamp(-1.0, 1.0) * 10_000.0) as i32;
        // Аппаратный таймер микросекунд переводим в миллисекунды (влезает в AtomicU32)
        let now_ms = (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u32;

        self.drive_forward.store(f, Ordering::Release);
        self.drive_turn.store(t, Ordering::Release);
        self.last_drive_ms.store(now_ms, Ordering::Release);
    }

    /// Чтение команды движения с проверкой Watchdog 300 мс (вызывается из контура 200 Гц на ядре 1).
    pub fn get_drive(&self) -> (f32, f32) {
        let now_ms = (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u32;
        let last_ms = self.last_drive_ms.load(Ordering::Acquire);

        // wrapping_sub корректно обрабатывает даже переполнение счетчика через 49 дней
        if now_ms.wrapping_sub(last_ms) > DRIVE_WATCHDOG_MS {
            return (0.0, 0.0);
        }

        let f = self.drive_forward.load(Ordering::Acquire) as f32 / 10_000.0;
        let t = self.drive_turn.load(Ordering::Acquire) as f32 / 10_000.0;
        (f, t)
    }
}