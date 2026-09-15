//! Телеметрия и черный ящик падений.

use std::sync::atomic::{AtomicI32, AtomicU8, AtomicU32, Ordering};
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
pub const EV_BAD_DT: u8 = 4;

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

pub struct Telemetry {
    angle: AtomicU32,
    accel_angle: AtomicU32,
    gyro: AtomicU32,
    accel_mag: AtomicU32,
    gyro_bias: AtomicU32,
    trim: AtomicU32,

    cmd: AtomicU32,
    wheel_rate: AtomicU32,
    term_p: AtomicU32,
    term_i: AtomicU32,
    term_d: AtomicU32,
    steps: AtomicI32,

    state: AtomicU8,
    imu_errors: AtomicU32,

    dt_us: AtomicU32,
    max_dt_us: AtomicU32,
    loop_us: AtomicU32,
    max_loop_us: AtomicU32,

    event: AtomicU8,
    event_val: AtomicU32,

    blackbox: Mutex<BlackBox>,
}

#[derive(Clone, Copy, Default)]
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

impl Telemetry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            angle: AtomicU32::new(0),
            accel_angle: AtomicU32::new(0),
            gyro: AtomicU32::new(0),
            accel_mag: AtomicU32::new(0),
            gyro_bias: AtomicU32::new(0),
            trim: AtomicU32::new(0),
            cmd: AtomicU32::new(0),
            wheel_rate: AtomicU32::new(0),
            term_p: AtomicU32::new(0),
            term_i: AtomicU32::new(0),
            term_d: AtomicU32::new(0),
            steps: AtomicI32::new(0),
            state: AtomicU8::new(0),
            imu_errors: AtomicU32::new(0),
            dt_us: AtomicU32::new(0),
            max_dt_us: AtomicU32::new(0),
            loop_us: AtomicU32::new(0),
            max_loop_us: AtomicU32::new(0),
            event: AtomicU8::new(0),
            event_val: AtomicU32::new(0),
            blackbox: Mutex::new(BlackBox::new()),
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            angle: Self::unpack(self.angle.load(Ordering::Relaxed)),
            accel_angle: Self::unpack(self.accel_angle.load(Ordering::Relaxed)),
            gyro: Self::unpack(self.gyro.load(Ordering::Relaxed)),
            accel_mag: Self::unpack(self.accel_mag.load(Ordering::Relaxed)),
            gyro_bias: Self::unpack(self.gyro_bias.load(Ordering::Relaxed)),
            trim: Self::unpack(self.trim.load(Ordering::Relaxed)),
            cmd: Self::unpack(self.cmd.load(Ordering::Relaxed)),
            wheel_rate: Self::unpack(self.wheel_rate.load(Ordering::Relaxed)),
            term_p: Self::unpack(self.term_p.load(Ordering::Relaxed)),
            term_lean: Self::unpack(self.term_i.load(Ordering::Relaxed)),
            term_d: Self::unpack(self.term_d.load(Ordering::Relaxed)),
            steps: self.steps.load(Ordering::Relaxed),
            state: self.state.load(Ordering::Relaxed),
            imu_errors: self.imu_errors.load(Ordering::Relaxed).min(u16::MAX as u32) as u16,
            dt_us: self.dt_us.load(Ordering::Relaxed).min(u16::MAX as u32) as u16,
            loop_us: self.loop_us.load(Ordering::Relaxed).min(u16::MAX as u32) as u16,
        }
    }

    fn pack(v: f32) -> u32 {
        if v.is_finite() { v.to_bits() } else { 0.0f32.to_bits() }
    }

    fn unpack(v: u32) -> f32 {
        f32::from_bits(v)
    }

    pub fn update_imu(&self, angle: f32, gyro: f32, accel_mag: f32, accel_angle: f32) {
        self.angle.store(Self::pack(angle), Ordering::Relaxed);
        self.gyro.store(Self::pack(gyro), Ordering::Relaxed);
        self.accel_mag.store(Self::pack(accel_mag), Ordering::Relaxed);
        self.accel_angle.store(Self::pack(accel_angle), Ordering::Relaxed);
    }

    pub fn update_control(&self, cmd: f32, wheel_rate: f32, p: f32, i: f32, d: f32, steps: i32) {
        self.cmd.store(Self::pack(cmd), Ordering::Relaxed);
        self.wheel_rate.store(Self::pack(wheel_rate), Ordering::Relaxed);
        self.term_p.store(Self::pack(p), Ordering::Relaxed);
        self.term_i.store(Self::pack(i), Ordering::Relaxed);
        self.term_d.store(Self::pack(d), Ordering::Relaxed);
        self.steps.store(steps, Ordering::Relaxed);
    }

    pub fn set_gyro_bias(&self, bias: f32) {
        self.gyro_bias.store(Self::pack(bias), Ordering::Relaxed);
    }

    pub fn set_trim(&self, trim: f32) {
        self.trim.store(Self::pack(trim), Ordering::Relaxed);
    }

    pub fn set_timing(&self, dt_us: u32, loop_us: u32) {
        self.dt_us.store(dt_us, Ordering::Relaxed);
        self.max_dt_us.fetch_max(dt_us, Ordering::Relaxed);
        self.loop_us.store(loop_us, Ordering::Relaxed);
        self.max_loop_us.fetch_max(loop_us, Ordering::Relaxed);
    }

    pub fn set_state(&self, state: u8) {
        self.state.store(state, Ordering::Relaxed);
    }

    pub fn set_imu_errors(&self, n: u32) {
        self.imu_errors.store(n, Ordering::Relaxed);
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
        struct LoggerCtx {
            tele: Arc<Telemetry>,
            period: Duration,
        }
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

unsafe extern "C" fn logger_entry(ctx: *mut core::ffi::c_void) {
    struct LoggerCtx {
        tele: Arc<Telemetry>,
        period: Duration,
    }
    let ctx = unsafe { Box::from_raw(ctx as *mut LoggerCtx) };
    let tele = ctx.tele;
    let period_ms = ctx.period.as_millis() as u32;
    let t0 = Instant::now();

    loop {
        FreeRtos::delay_ms(period_ms);

        let t = t0.elapsed().as_secs_f32();
        let state = tele.state.load(Ordering::Relaxed);
        let state_str = if state == STATE_BALANCING { "BAL " } else { "IDLE" };

        let angle = Telemetry::unpack(tele.angle.load(Ordering::Relaxed));
        let accel_angle = Telemetry::unpack(tele.accel_angle.load(Ordering::Relaxed));
        let gyro = Telemetry::unpack(tele.gyro.load(Ordering::Relaxed));
        let mag = Telemetry::unpack(tele.accel_mag.load(Ordering::Relaxed));
        let bias = Telemetry::unpack(tele.gyro_bias.load(Ordering::Relaxed));
        let trim = Telemetry::unpack(tele.trim.load(Ordering::Relaxed));

        let cmd = Telemetry::unpack(tele.cmd.load(Ordering::Relaxed));
        let wheel_rate = Telemetry::unpack(tele.wheel_rate.load(Ordering::Relaxed));
        let p = Telemetry::unpack(tele.term_p.load(Ordering::Relaxed));
        let i = Telemetry::unpack(tele.term_i.load(Ordering::Relaxed));
        let d = Telemetry::unpack(tele.term_d.load(Ordering::Relaxed));
        let steps = tele.steps.load(Ordering::Relaxed);

        let dt_us = tele.dt_us.load(Ordering::Relaxed);
        let max_dt_us = tele.max_dt_us.swap(0, Ordering::Relaxed);
        let loop_us = tele.loop_us.load(Ordering::Relaxed);
        let max_loop_us = tele.max_loop_us.swap(0, Ordering::Relaxed);
        let imu_errors = tele.imu_errors.load(Ordering::Relaxed);

        log::info!(
            "[{:6.1}s] {} A={:+.2}(acc{:+.2}) G={:+.1} B={:+.2} T={:+.2} M={:.2} | CMD={:+.0} W={:+.0} S={} | P={:+.0} lean={:+.2} D={:+.0} | dt={}({})us L={}({})us IE={}",
            t, state_str, angle, accel_angle, gyro, bias, trim, mag,
            cmd, wheel_rate, steps,
            p, i, d,
            dt_us, max_dt_us, loop_us, max_loop_us,
            imu_errors
        );

        let ev = tele.event.swap(EV_NONE, Ordering::Relaxed);
        if ev != EV_NONE {
            let raw = tele.event_val.load(Ordering::Relaxed) as i32 as f32 / 100.0;
            match ev {
                EV_ARMED => log::info!(">>> БАЛАНС ВКЛ (угол {:+.2}°)", raw),
                EV_FALL => log::warn!(">>> ПАДЕНИЕ (угол {:+.2}°) — чёрный ящик сохранён", raw),
                EV_IMU_DEAD => log::error!(">>> MPU6050 НЕ ОТВЕЧАЕТ — СТОП"),
                EV_BAD_DT => log::warn!(">>> СКАЧОК dt до {:.2} мс — АВАРИЙНЫЙ СТОП", raw),
                _ => {}
            }
        }
    }
}