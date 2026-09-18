//! Телеметрия и черный ящик падений.
//!
//! Обмен между контуром (ядро 1) и сетевыми задачами (ядро 0) устроен
//! по схеме seqlock: один разделяемый `Snapshot` в общей памяти.
//! Писатель (контур, единственный) перед правкой инкрементирует счётчик
//! последовательности до нечётного, правит структуру, инкрементирует
//! до чётного. Читатель копирует структуру целиком и проверяет, что
//! счётчик не изменился, — иначе повторяет. Итог: ни упаковки
//! f32 → u32 → f32, ни 17 отдельных атомиков, ни разорванных кадров
//! из разных циклов контура.

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::sys;

pub const STATE_IDLE: u8 = 0;
pub const STATE_BALANCING: u8 = 1;

pub const EV_NONE: u8 = 0;
pub const EV_ARMED: u8 = 1;
pub const EV_FALL: u8 = 2;
pub const EV_IMU_DEAD: u8 = 3;
pub const EV_ARM_FAIL: u8 = 4;
pub const EV_STABILITY_WARN: u8 = 7;

const LOGGER_STACK: u32 = 4096;
const LOGGER_PRIORITY: u32 = 2;
const LOGGER_CORE: i32 = 0;

pub const BLACKBOX_CAPACITY: usize = 400; // 2 секунды при 200 Гц

#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct BlackBoxFrame {
    pub t_ms: u32,
    pub angle: f32,
    pub gyro: f32,
    pub cmd: f32,
    pub wheel_rate: f32,
    pub steps: i32,
    pub dt_us: u16,
    pub pid_ka: f32,
    pub pid_kd: f32,
    pub pid_ki: f32,
    pub flags: u8,
}

struct BlackBox {
    frames: Vec<BlackBoxFrame>,
    head: usize,
    count: usize,
    frozen: bool,
}

impl BlackBox {
    fn new() -> Self {
        Self {
            frames: vec![BlackBoxFrame::default(); BLACKBOX_CAPACITY],
            head: 0,
            count: 0,
            frozen: false,
        }
    }

    fn push(&mut self, frame: BlackBoxFrame) {
        if self.frozen {
            return;
        }
        self.frames[self.head] = frame;
        self.head = (self.head + 1) % BLACKBOX_CAPACITY;
        if self.count < BLACKBOX_CAPACITY {
            self.count += 1;
        }
    }

    fn freeze(&mut self) {
        self.frozen = true;
    }

    fn unfreeze(&mut self) {
        self.frozen = false;
        self.head = 0;
        self.count = 0;
    }

    fn to_json(&self) -> serde_json::Value {
        let mut list = Vec::with_capacity(self.count);
        if self.count < BLACKBOX_CAPACITY {
            for i in 0..self.count {
                list.push(self.frames[i]);
            }
        } else {
            for i in 0..BLACKBOX_CAPACITY {
                let idx = (self.head + i) % BLACKBOX_CAPACITY;
                list.push(self.frames[idx]);
            }
        }
        serde_json::to_value(list).unwrap_or_else(|_| serde_json::json!([]))
    }
}

/// Кадр телеметрии: лежит в общей памяти как есть, без упаковки.
/// Выравнивание 4 байта — все поля f32/i32 читаются атомарно по одному.
#[derive(Clone, Copy, Default)]
#[repr(align(4))]
pub struct Snapshot {
    pub angle: f32,
    pub accel_angle: f32,
    pub gyro: f32,
    pub accel_mag: f32,
    pub gyro_bias: f32,
    pub trim: f32,
    pub cmd: f32,
    pub wheel_rate: f32,
    pub term_p: f32,
    pub term_lean: f32,
    pub term_d: f32,
    pub steps: i32,
    pub state: u8,
    pub imu_errors: u16,
    pub dt_us: u16,
    pub loop_us: u16,
}

pub struct Telemetry {
    /// Счётчик seqlock: чётный — данные стабильны, нечётный — идёт запись.
    seq: AtomicU32,
    /// Разделяемый кадр. Безопасен через seqlock: писатель один (контур),
    /// читатели копируют структуру только при стабильном счётчике.
    data: std::cell::UnsafeCell<Snapshot>,

    // Статистика максимумов для логгера (в UDP-пакет не входит)
    max_dt_us: AtomicU32,
    max_loop_us: AtomicU32,

    // События: код + значение — дешёвые целочисленные атомики без упаковки
    event: AtomicU8,
    event_val: AtomicU32,

    blackbox: Mutex<BlackBox>,
}

// Безопасность: `data` защищена протоколом seqlock (единственный писатель —
// контур управления; читатели не трогают данные при нечётном счётчике и
// перечитывают при его смене). Остальные поля — обычные Sync-типы.
unsafe impl Sync for Telemetry {}

impl Telemetry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            seq: AtomicU32::new(0),
            data: std::cell::UnsafeCell::new(Snapshot::default()),
            max_dt_us: AtomicU32::new(0),
            max_loop_us: AtomicU32::new(0),
            event: AtomicU8::new(EV_NONE),
            event_val: AtomicU32::new(0),
            blackbox: Mutex::new(BlackBox::new()),
        })
    }

    /// Запись в разделяемый кадр. Вызывается только из контура (один писатель).
    fn write(&self, f: impl FnOnce(&mut Snapshot)) {
        self.seq.fetch_add(1, Ordering::AcqRel); // нечётное — читатели ждут
        let cell = unsafe { &mut *self.data.get() };
        f(cell);
        self.seq.fetch_add(1, Ordering::AcqRel); // чётное — кадр опубликован
    }

    /// Консистентная копия кадра: без упаковки, одним чтением структуры.
    pub fn snapshot(&self) -> Snapshot {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 == 0 {
                let copy = unsafe { core::ptr::read_volatile(self.data.get()) };
                if self.seq.load(Ordering::Acquire) == before {
                    return copy;
                }
            }
            // Писатель держит кадр или уже перезаписал — повторяем
        }
    }

    fn fin(v: f32) -> f32 {
        if v.is_finite() { v } else { 0.0 }
    }

    pub fn update_imu(&self, angle: f32, gyro: f32, accel_mag: f32, accel_angle: f32) {
        self.write(|s| {
            s.angle = Self::fin(angle);
            s.gyro = Self::fin(gyro);
            s.accel_mag = Self::fin(accel_mag);
            s.accel_angle = Self::fin(accel_angle);
        });
    }

    pub fn update_control(&self, cmd: f32, wheel_rate: f32, p: f32, i: f32, d: f32, steps: i32) {
        self.write(|s| {
            s.cmd = Self::fin(cmd);
            s.wheel_rate = Self::fin(wheel_rate);
            s.term_p = Self::fin(p);
            s.term_lean = Self::fin(i);
            s.term_d = Self::fin(d);
            s.steps = steps;
        });
    }

    pub fn set_gyro_bias(&self, bias: f32) {
        self.write(|s| s.gyro_bias = Self::fin(bias));
    }

    pub fn set_trim(&self, trim: f32) {
        self.write(|s| s.trim = Self::fin(trim));
    }

    pub fn set_timing(&self, dt_us: u32, loop_us: u32) {
        self.write(|s| {
            s.dt_us = dt_us.min(u16::MAX as u32) as u16;
            s.loop_us = loop_us.min(u16::MAX as u32) as u16;
        });
        self.max_dt_us.fetch_max(dt_us, Ordering::Relaxed);
        self.max_loop_us.fetch_max(loop_us, Ordering::Relaxed);
    }

    pub fn set_state(&self, state: u8) {
        self.write(|s| s.state = state);
    }

    pub fn set_imu_errors(&self, n: u32) {
        self.write(|s| s.imu_errors = n.min(u16::MAX as u32) as u16);
    }

    pub fn send_event(&self, event: u8, val: f32) {
        self.event_val.store((val * 100.0) as i32 as u32, Ordering::Relaxed);
        self.event.store(event, Ordering::Relaxed);
    }

    pub fn record_blackbox(&self, frame: BlackBoxFrame) {
        if let Ok(mut bb) = self.blackbox.lock() {
            bb.push(frame);
        }
    }

    pub fn freeze_blackbox(&self) {
        if let Ok(mut bb) = self.blackbox.lock() {
            bb.freeze();
        }
    }

    pub fn reset_blackbox(&self) {
        if let Ok(mut bb) = self.blackbox.lock() {
            bb.unfreeze();
        }
    }

    pub fn get_blackbox_json(&self) -> serde_json::Value {
        self.blackbox
            .lock()
            .map(|bb| bb.to_json())
            .unwrap_or_else(|_| serde_json::json!([]))
    }

    pub fn spawn_logger(self: &Arc<Self>, period: Duration) -> anyhow::Result<()> {
        let ctx = Box::into_raw(Box::new(LoggerCtx {
            tele: self.clone(),
            period,
        })) as *mut core::ffi::c_void;

        let created = unsafe {
            sys::xTaskCreatePinnedToCore(
                Some(logger_entry),
                b"logger\0".as_ptr() as *const _,
                LOGGER_STACK,
                ctx,
                LOGGER_PRIORITY,
                core::ptr::null_mut(),
                LOGGER_CORE,
            )
        };

        if created != 1 {
            unsafe { drop(Box::from_raw(ctx as *mut LoggerCtx)); }
            anyhow::bail!("Не удалось создать задачу логгера");
        }

        Ok(())
    }
}

/// Контекст задачи логгера: уходит в FreeRTOS как сырой указатель и
/// восстанавливается в `logger_entry`. Объявлен один раз, чтобы layout
/// не разъехался между созданием и чтением.
struct LoggerCtx {
    tele: Arc<Telemetry>,
    period: Duration,
}

unsafe extern "C" fn logger_entry(ctx: *mut core::ffi::c_void) {
    let ctx = unsafe { Box::from_raw(ctx as *mut LoggerCtx) };
    let tele = ctx.tele;
    let period_ms = ctx.period.as_millis() as u32;
    let t0 = Instant::now();

    loop {
        FreeRtos::delay_ms(period_ms);

        let t = t0.elapsed().as_secs_f32();
        let s = tele.snapshot();
        let state_str = if s.state == STATE_BALANCING { "BAL " } else { "IDLE" };

        let max_dt_us = tele.max_dt_us.swap(0, Ordering::Relaxed);
        let max_loop_us = tele.max_loop_us.swap(0, Ordering::Relaxed);

        log::info!(
            "[{:6.1}s] {} A={:+.2}(acc{:+.2}) G={:+.1} B={:+.2} T={:+.2} M={:.2} | CMD={:+.0} W={:+.0} S={} | P={:+.0} lean={:+.2} D={:+.0} | dt={}({})us L={}({})us IE={}",
            t, state_str, s.angle, s.accel_angle, s.gyro, s.gyro_bias, s.trim, s.accel_mag,
            s.cmd, s.wheel_rate, s.steps,
            s.term_p, s.term_lean, s.term_d,
            s.dt_us, max_dt_us, s.loop_us, max_loop_us,
            s.imu_errors
        );

        let ev = tele.event.swap(EV_NONE, Ordering::Relaxed);
        if ev != EV_NONE {
            let raw = tele.event_val.load(Ordering::Relaxed) as i32 as f32 / 100.0;
            match ev {
                EV_ARMED => log::info!(">>> БАЛАНС ВКЛ (угол {:+.2}°)", raw),
                EV_FALL => log::warn!(">>> ПАДЕНИЕ (угол {:+.2}°) — чёрный ящик сохранён", raw),
                EV_IMU_DEAD => log::error!(">>> MPU6050 НЕ ОТВЕЧАЕТ — СТОП"),
                EV_ARM_FAIL => log::warn!(">>> НЕУДАЧНЫЙ АРМИНГ (угол {:+.2}°) — стоп", raw),
                EV_STABILITY_WARN => {
                    log::warn!(">>> ПОТЕРЯ УСТОЙЧИВОСТИ (ошибка {:+.2}°) — близко к падению", raw)
                }
                _ => {}
            }
        }
    }
}