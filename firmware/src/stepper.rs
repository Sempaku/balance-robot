use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::{Ets, FreeRtos};
use esp_idf_svc::hal::gpio::{Output, PinDriver};
use esp_idf_svc::sys::EspError;

pub const MIN_STEP_RATE: f32 = 10.0;

/// Микрошаг на A4988. Ножки MS1, MS2, MS3 запаяны на 3.3V = 1/16 микрошага.
pub const MICROSTEPS: f32 = 16.0;
pub const STEPS_PER_REV: f32 = 200.0 * MICROSTEPS; // 3200 шагов на оборот

/// Максимум оборотов колеса в секунду.
pub const MAX_WHEEL_RPS: f32 = 3.0;
pub const MAX_STEP_RATE: f32 = STEPS_PER_REV * MAX_WHEEL_RPS; // 9600 шаг/с

/// Ограничение ускорения колеса (шаг/с²).
const RATE_ACCEL_LIMIT: f32 = 120_000.0;
const HEARTBEAT_TIMEOUT: Duration = Duration::from_millis(100);

const TASK_STACK_SIZE: u32 = 8192;
const TASK_PRIORITY: u32 = 10;
const TASK_CORE: i32 = 1;

pub struct StepperMotor<'d> {
    step_pin: PinDriver<'d, Output>,
    dir_pin: PinDriver<'d, Output>,
}

impl<'d> StepperMotor<'d> {
    pub fn new(step_pin: PinDriver<'d, Output>, dir_pin: PinDriver<'d, Output>) -> Self {
        Self { step_pin, dir_pin }
    }

    #[inline(always)]
    pub fn set_direction(&mut self, forward: bool) -> Result<(), EspError> {
        if forward {
            self.dir_pin.set_high()
        } else {
            self.dir_pin.set_low()
        }
    }

    #[inline(always)]
    pub fn set_step_high(&mut self) -> Result<(), EspError> {
        self.step_pin.set_high()
    }

    #[inline(always)]
    pub fn set_step_low(&mut self) -> Result<(), EspError> {
        self.step_pin.set_low()
    }
}

struct EngineCtx {
    left: StepperMotor<'static>,
    right: StepperMotor<'static>,
    left_cmd: Arc<AtomicU32>,
    right_cmd: Arc<AtomicU32>,
    actual_left: Arc<AtomicU32>,
    actual_right: Arc<AtomicU32>,
    seq: Arc<AtomicU32>,
    left_steps: Arc<AtomicI32>,
    right_steps: Arc<AtomicI32>,
}

pub struct StepperEngine {
    left_cmd: Arc<AtomicU32>,
    right_cmd: Arc<AtomicU32>,
    actual_left: Arc<AtomicU32>,
    actual_right: Arc<AtomicU32>,
    cmd_seq: Arc<AtomicU32>,
    left_steps: Arc<AtomicI32>,
    right_steps: Arc<AtomicI32>,
}

impl StepperEngine {
    pub fn spawn(
        left: StepperMotor<'static>,
        right: StepperMotor<'static>,
    ) -> anyhow::Result<Self> {
        let left_cmd = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let right_cmd = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let actual_left = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let actual_right = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let cmd_seq = Arc::new(AtomicU32::new(0));
        let left_steps = Arc::new(AtomicI32::new(0));
        let right_steps = Arc::new(AtomicI32::new(0));

        let ctx = Box::into_raw(Box::new(EngineCtx {
            left,
            right,
            left_cmd: left_cmd.clone(),
            right_cmd: right_cmd.clone(),
            actual_left: actual_left.clone(),
            actual_right: actual_right.clone(),
            seq: cmd_seq.clone(),
            left_steps: left_steps.clone(),
            right_steps: right_steps.clone(),
        })) as *mut core::ffi::c_void;

        let created = unsafe {
            esp_idf_svc::sys::xTaskCreatePinnedToCore(
                Some(stepper_entry),
                b"stepper\0".as_ptr() as *const _,
                TASK_STACK_SIZE,
                ctx,
                TASK_PRIORITY,
                core::ptr::null_mut(),
                TASK_CORE,
            )
        };

        if created != 1 {
            unsafe {
                drop(Box::from_raw(ctx as *mut EngineCtx));
            }
            anyhow::bail!("Не удалось создать задачу шаговика");
        }

        Ok(Self {
            left_cmd,
            right_cmd,
            actual_left,
            actual_right,
            cmd_seq,
            left_steps,
            right_steps,
        })
    }

    /// Задание раздельных скоростей для колес (шаг/с).
    pub fn set_rates(&self, left: f32, right: f32) {
        let l = if left.is_finite() {
            left.clamp(-MAX_STEP_RATE, MAX_STEP_RATE)
        } else {
            0.0
        };
        let r = if right.is_finite() {
            right.clamp(-MAX_STEP_RATE, MAX_STEP_RATE)
        } else {
            0.0
        };
        self.left_cmd.store(l.to_bits(), Ordering::Relaxed);
        self.right_cmd.store(r.to_bits(), Ordering::Relaxed);
        self.cmd_seq.fetch_add(1, Ordering::Relaxed);
    }

    /// Задание общей скорости (для обратной совместимости).
    pub fn set_rate(&self, rate: f32) {
        self.set_rates(rate, rate);
    }

    pub fn stop(&self) {
        self.set_rates(0.0, 0.0);
    }

    /// Среднее пройденное расстояние в шагах.
    pub fn steps(&self) -> i32 {
        let l = self.left_steps.load(Ordering::Relaxed);
        let r = self.right_steps.load(Ordering::Relaxed);
        (l + r) / 2
    }

    /// Средняя фактическая скорость колес (шаг/с).
    pub fn actual_rate(&self) -> f32 {
        let l = f32::from_bits(self.actual_left.load(Ordering::Relaxed));
        let r = f32::from_bits(self.actual_right.load(Ordering::Relaxed));
        (l + r) * 0.5
    }
}

unsafe extern "C" fn stepper_entry(ctx: *mut core::ffi::c_void) {
    let ctx = unsafe { Box::from_raw(ctx as *mut EngineCtx) };
    let core = unsafe { esp_idf_svc::sys::xTaskGetCoreID(core::ptr::null_mut()) };
    log::info!("Stepper: двухмоторная задача запущена (ядро {})", core);
    stepper_loop(*ctx);
    park();
}

fn park() -> ! {
    loop {
        FreeRtos::delay_ms(10_000);
    }
}

fn stepper_loop(ctx: EngineCtx) {
    let EngineCtx {
        mut left,
        mut right,
        left_cmd,
        right_cmd,
        actual_left,
        actual_right,
        seq: cmd_seq,
        left_steps,
        right_steps,
    } = ctx;

    let mut cur_left = 0.0f32;
    let mut cur_right = 0.0f32;

    let mut fwd_left = false;
    let mut fwd_right = false;
    let mut dirs_init = false;

    let mut last_seq = 0u32;
    let mut last_cmd_time = Instant::now();
    let mut heartbeat_lost = false;
    let mut last_rate_update = Instant::now();

    let mut next_step_left = Instant::now();
    let mut next_step_right = Instant::now();

    loop {
        let now = Instant::now();

        let seq = cmd_seq.load(Ordering::Relaxed);
        if seq != last_seq {
            last_seq = seq;
            last_cmd_time = now;
            heartbeat_lost = false;
        }

        let mut tgt_left = f32::from_bits(left_cmd.load(Ordering::Relaxed));
        let mut tgt_right = f32::from_bits(right_cmd.load(Ordering::Relaxed));

        // Heartbeat watchdog от главного контура
        if (tgt_left != 0.0 || tgt_right != 0.0 || cur_left != 0.0 || cur_right != 0.0)
            && now.duration_since(last_cmd_time) > HEARTBEAT_TIMEOUT
        {
            if !heartbeat_lost {
                log::warn!("Stepper: нет команд от control — остановка");
                heartbeat_lost = true;
            }
            tgt_left = 0.0;
            tgt_right = 0.0;
        }

        // Плавное ограничение ускорения
        let dt = now.duration_since(last_rate_update).as_secs_f32();
        last_rate_update = now;

        if dt.is_finite() && dt > 0.0 && dt < 0.1 {
            let max_delta = RATE_ACCEL_LIMIT * dt;
            cur_left += (tgt_left - cur_left).clamp(-max_delta, max_delta);
            cur_right += (tgt_right - cur_right).clamp(-max_delta, max_delta);
        } else {
            cur_left = tgt_left;
            cur_right = tgt_right;
        }

        if cur_left.abs() < MIN_STEP_RATE && tgt_left.abs() < MIN_STEP_RATE {
            cur_left = 0.0;
        }
        if cur_right.abs() < MIN_STEP_RATE && tgt_right.abs() < MIN_STEP_RATE {
            cur_right = 0.0;
        }

        actual_left.store(cur_left.to_bits(), Ordering::Relaxed);
        actual_right.store(cur_right.to_bits(), Ordering::Relaxed);

        if cur_left == 0.0 && cur_right == 0.0 {
            FreeRtos::delay_ms(1);
            next_step_left = Instant::now();
            next_step_right = Instant::now();
            continue;
        }

        // Обновление направления DIR пинов
        // Левый мотор: forward при отрицательной скорости
        // Правый мотор: зеркальный, forward при противоположном знаке
        let want_fwd_left = cur_left < 0.0;
        let want_fwd_right = !(cur_right < 0.0);

        if !dirs_init || want_fwd_left != fwd_left || want_fwd_right != fwd_right {
            fwd_left = want_fwd_left;
            fwd_right = want_fwd_right;
            dirs_init = true;

            if left.set_direction(fwd_left).is_err() || right.set_direction(fwd_right).is_err() {
                log::error!("Stepper: ошибка DIR — задача остановлена");
                park();
            }
            Ets::delay_us(2);
        }

        let period_left = if cur_left != 0.0 {
            Duration::from_micros((1_000_000.0 / cur_left.abs().min(MAX_STEP_RATE)) as u64)
        } else {
            Duration::from_secs(10)
        };

        let period_right = if cur_right != 0.0 {
            Duration::from_micros((1_000_000.0 / cur_right.abs().min(MAX_STEP_RATE)) as u64)
        } else {
            Duration::from_secs(10)
        };

        let step_l = cur_left != 0.0 && now >= next_step_left;
        let step_r = cur_right != 0.0 && now >= next_step_right;

        if step_l || step_r {
            // Параллельный подъем пинов STEP
            if step_l {
                let _ = left.set_step_high();
                let signed = if fwd_left { -1 } else { 1 };
                left_steps.fetch_add(signed, Ordering::Relaxed);
                next_step_left += period_left;
                if next_step_left <= now {
                    next_step_left = now + period_left;
                }
            }
            if step_r {
                let _ = right.set_step_high();
                let signed = if fwd_right { 1 } else { -1 };
                right_steps.fetch_add(signed, Ordering::Relaxed);
                next_step_right += period_right;
                if next_step_right <= now {
                    next_step_right = now + period_right;
                }
            }

            // Аппаратный импульс A4988 (>= 1 мкс)
            Ets::delay_us(2);

            if step_l {
                let _ = left.set_step_low();
            }
            if step_r {
                let _ = right.set_step_low();
            }
        }

        // Ожидание ближайшего события следующего шага
        let next_wake = match (cur_left != 0.0, cur_right != 0.0) {
            (true, true) => next_step_left.min(next_step_right),
            (true, false) => next_step_left,
            (false, true) => next_step_right,
            (false, false) => now + Duration::from_millis(1),
        };

        let rem = next_wake.saturating_duration_since(Instant::now());
        if rem > Duration::from_millis(3) {
            FreeRtos::delay_ms((rem.as_millis() - 2) as u32);
        }
        while Instant::now() < next_wake {}
    }
}