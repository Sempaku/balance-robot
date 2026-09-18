mod control;
mod filter;
mod mpu6050;
mod net;
mod params;
#[allow(dead_code)]
mod pid;
mod remote;
mod signcheck;
mod stepper;
mod storage;
mod telemetry;
mod tuning;
mod autotune;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{Output, PinDriver};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::task::notification::Notification;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use esp_idf_svc::hal::units::FromValueType;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::timer::{EspTimerService, Task as TimerDispatch};

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// Глобальный флаг запроса калибровки гироскопа
static CALIBRATE_REQUEST: AtomicBool = AtomicBool::new(false);

/// Запросить калибровку гироскопа на ходу.
/// Вызывается из UART-консоли или других задач.
pub fn request_gyro_calibration() {
    CALIBRATE_REQUEST.store(true, Ordering::Relaxed);
}

// Глобальный флаг запроса автонастройки PID
static AUTOTUNE_REQUEST: AtomicBool = AtomicBool::new(false);

/// Запросить автонастройку PID.
pub fn request_autotune() {
    AUTOTUNE_REQUEST.store(true, Ordering::Relaxed);
}

use control::{BalanceController, FallDetector, MOTOR_SIGN};
use filter::ComplementaryFilter;
use mpu6050::{Mpu6050, CALIBRATION_MAX_SPREAD_DPS};
use net::{NetStatus, PORT_COMMAND, PORT_TELEMETRY, http_api::HTTP_PORT};
use params::{ParamStore, Params};
use remote::RemoteState;
use signcheck::GyroSignCheck;
use stepper::{StepperEngine, StepperMotor, MAX_STEP_RATE, MICROSTEPS};
use storage::Storage;
use telemetry::{
    BlackBoxFrame, Telemetry, EV_ARMED, EV_ARM_FAIL, EV_FALL, EV_IMU_DEAD, EV_STABILITY_WARN,
    STATE_BALANCING, STATE_IDLE,
};
use autotune::RelayTuner;

const TELEMETRY_PERIOD: Duration = Duration::from_millis(200);

// Реалистичные пороги для удержания робота руками человека
const ARM_MAX_GYRO_DPS: f32 = 16.0;  // Тремор рук обычно 8-12°/с
const ARM_MAX_RATE: f32 = 60.0;
const ARM_STABLE_CYCLES: u32 = 60;   // 60 × 5 мс = 300 мс неподвижности (вместо 500 мс)
const MAX_IMU_ERRORS: u32 = 3;
const OTA_CONFIRM_AFTER: Duration = Duration::from_secs(30);
const MOTOR_TEST_ON_BOOT: bool = false;

/// Защита от убегания при арминге: если через 200 мс после включения баланса
/// угол > 15° ИЛИ гироскоп > 100°/с — аварийный стоп.
const ARM_FAIL_CHECK_MS: u32 = 200;
const ARM_FAIL_MAX_ANGLE: f32 = 15.0;
const ARM_FAIL_MAX_GYRO: f32 = 100.0;
const ARM_FAIL_COOLDOWN_MS: u32 = 2000; // Блокировка повторного арминга 2 сек

/// Период цикла управления: 5 мс = 200 Гц.
const CONTROL_PERIOD: Duration = Duration::from_millis(5);

/// Автокалибровка гироскопа на неподвижном роботе: дрейф нуля
/// (температурный, у клонов MPU доходит до ~8°/с) не должен наматывать угол.
const AUTOCAL_STABLE_CYCLES: u32 = 300; // 1.5 с неподвижности перед калибровкой
const AUTOCAL_DRIFT_DPS: f32 = 1.5;     // выше этого считаем дрейф значимым
const AUTOCAL_ACCEL_SPREAD_DEG: f32 = 2.0; // корпус не наклоняют
const AUTOCAL_COOLDOWN: Duration = Duration::from_secs(10);

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    ThreadSpawnConfiguration {
        name: Some(c"control"),
        stack_size: 24 * 1024,
        priority: 11,
        pin_to_core: Some(Core::Core1),
        ..Default::default()
    }
    .set()
    .expect("не удалось настроить задачу control");

    let control = std::thread::Builder::new()
        .name("control".into())
        .spawn(robot_main)
        .expect("не удалось создать задачу control");

    let _ = ThreadSpawnConfiguration::default().set();

    let result = control.join().expect("control паника");
    if let Err(e) = result {
        panic!("control: {e:?}");
    }
}

fn robot_main() -> anyhow::Result<()> {
    log::info!("=========================================");
    log::info!(" ROBOT BALANCER — cascaded PD");
    log::info!(" MOTOR_SIGN={MOTOR_SIGN}  микрошаг 1/{MICROSTEPS}");
    log::info!("=========================================");

    // Watchdog: программный таймер для детектирования зависаний
    // Примечание: аппаратный ESP-IDF WDT требует настройки в sdkconfig
    let mut wdt_timer = Instant::now();

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs_part = EspDefaultNvsPartition::take()?;

    let params = ParamStore::new(Params::default());
    let storage = Arc::new(Storage::new(nvs_part.clone())?);
    match storage.load_params(&params) {
        0 => log::info!("Параметры: из NVS ничего не взято, работаю на зашитых"),
        n => log::info!("Параметры: из NVS применено {n} значений"),
    }

    let i2c_config = I2cConfig::new().baudrate(400.kHz().into());
    let i2c = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio21,
        peripherals.pins.gpio22,
        &i2c_config,
    )?;
    log::info!("I2C: 400 kHz, SDA=GPIO21, SCL=GPIO22");

    let mut imu = Mpu6050::new(i2c)?;

    let left = StepperMotor::new(
        PinDriver::output(peripherals.pins.gpio19)?,
        PinDriver::output(peripherals.pins.gpio18)?,
    );
    let right = StepperMotor::new(
        PinDriver::output(peripherals.pins.gpio26)?,
        PinDriver::output(peripherals.pins.gpio25)?,
    );

    let engine = StepperEngine::spawn(left, right)?;
    log::info!("Stepper engine на ядре 1, max {MAX_STEP_RATE:.0} шаг/с");

    let tele = Telemetry::new();
    tele.spawn_logger(TELEMETRY_PERIOD)?;

    tuning::spawn(&params, &storage);

    let net_status = Arc::new(NetStatus::new());
    let remote = Arc::new(RemoteState::new());
    start_network(
        peripherals.modem,
        sysloop,
        nvs_part,
        &storage,
        &params,
        &tele,
        &net_status,
        &remote,
    );

    let mut tunables = params.snapshot();
    let mut filter = ComplementaryFilter::new(tunables.filter_tau);
    let mut ctrl = BalanceController::new(tunables.gains());
    let mut fall_detector = FallDetector::new();
    let mut sign_check = GyroSignCheck::new();
    let mut led = PinDriver::output(peripherals.pins.gpio2)?;
    led.set_low()?;
    
    let mut autotuner = RelayTuner::new();

    if MOTOR_TEST_ON_BOOT {
        motor_direction_test(&engine);
    }

    let initial_accel_angle = wait_imu_angle(&mut imu)?;
    filter.reset(initial_accel_angle);

    log::info!("Начальный угол IMU: {initial_accel_angle:.2}°");
    log::info!("Держи робота примерно вертикально (|A|<{:.0}°), синий LED включит баланс.", tunables.arm_max_angle);

    let mut is_balancing = false;
    let mut imu_errors = 0u32;
    let mut arm_stable = 0u32;
    let mut trim_sum = 0.0f32;
    let mut last_steps = engine.steps();

    // Защита от убегания: таймер и флаг неудачного арминга
    let mut arm_fail_timer: Option<Instant> = None;
    let mut arm_fail_cooldown_until: Option<Instant> = None;

    // Автокалибровка гироскопа: окно неподвижности и cooldown
    let mut cal_stable: u32 = 0;
    let mut cal_accel_min = f32::MAX;
    let mut cal_accel_max = f32::MIN;
    let mut cal_cooldown_until: Option<Instant> = None;

    let boot = Instant::now();
    let mut ota_confirmed = false;
    let mut last_imu_time = Instant::now();

    // Аппаратный такт контура управления: esp_timer с микросекундным разрешением.
    // xTaskDelayUntil квантуется тиком FreeRTOS (1 мс) и даёт джиттер периода
    // до целого тика; esp_timer пробуждает контур напрямую через task
    // notification из высокоприоритетной задачи таймера.
    // Notification привязывается к текущей задаче — создаётся здесь, в контуре.
    let tick = Arc::new(Notification::new());
    let tick_notifier = tick.notifier();
    let timer_service = EspTimerService::<TimerDispatch>::new()?;
    let tick_timer = timer_service.timer(move || {
        let _ = unsafe { tick_notifier.notify(NonZeroU32::new(1).unwrap()) };
    })?;
    tick_timer.every(CONTROL_PERIOD)?;
    log::info!("Контур: такт от esp_timer каждые 5 мс (разрешение 1 мкс)");

    loop {
        tick.wait_any();

        let now = Instant::now();
        let raw_dt = now.duration_since(last_imu_time).as_secs_f32();
        last_imu_time = now;
        if !raw_dt.is_finite() || raw_dt <= 0.0 {
            continue;
        }
        let dt = raw_dt.clamp(0.002, 0.020);
        let loop_t0 = Instant::now();
        let uptime_ms = boot.elapsed().as_millis() as u32;

        tunables = params.snapshot();
        ctrl.set_gains(tunables.gains());
        filter.set_tau(tunables.filter_tau);

        if !ota_confirmed && boot.elapsed() > OTA_CONFIRM_AFTER {
            ota_confirmed = true;
            net::ota::confirm_running_image();
        }

        // Watchdog: детектируем зависание если не кормим таймер > 3 сек
        if wdt_timer.elapsed() > Duration::from_millis(3000) {
            log::error!("WATCHDOG: зависание детектировано!");
            // Примечание: для аппаратной перезагрузки нужен ESP-IDF WDT
            // Здесь только логирование
        }
        // Кормим таймер каждый цикл (200 Гц = каждые 5 мс)
        wdt_timer = Instant::now();

        match imu.read() {
            Ok(data) => {
                imu_errors = 0;
                tele.set_imu_errors(0);

                let gyro = data.gyro_rate_dps;
                let wheel_rate = engine.actual_rate();

                // Обработка запроса автонастройки: из UART или из сети (кнопка в UI)
                let tune_requested =
                    AUTOTUNE_REQUEST.swap(false, Ordering::Relaxed) | remote.take_autotune();
                if tune_requested && !is_balancing {
                    log::info!("AUTOTUNE: запуск... ДЕРЖИ РОБОТА ВЕРТИКАЛЬНО!");
                    remote.set_autotune_state(remote::AUTOTUNE_RUNNING);
                    autotuner.start();
                }
                
                // Обработка запроса калибровки гироскопа
                if CALIBRATE_REQUEST.swap(false, Ordering::Relaxed) {
                    if wheel_rate.abs() < 50.0 && gyro.abs() < 20.0 {
                        log::info!("Калибровка гироскопа на ходу — НЕ ДВИГАЙ РОБОТА ~1 с");
                        match imu.calibrate_with_result() {
                            Ok(spread) if spread <= CALIBRATION_MAX_SPREAD_DPS => {
                                // Привязываем фильтр к акселерометру: старый угол
                                // мог намотаться от дрейфа до калибровки
                                filter.reset(data.accel_angle_deg);
                                filter.reset_bias();
                                log::info!("Калибровка успешна: offset={:+.3} °/с, spread={:.2} °/с",
                                    imu.offset(), spread);
                            }
                            Ok(spread) => {
                                log::warn!("Калибровка шумная: spread={:.2} °/с (порог {:.2})", 
                                    spread, CALIBRATION_MAX_SPREAD_DPS);
                            }
                            Err(e) => {
                                log::error!("Ошибка калибровки: {e:?}");
                            }
                        }
                    } else {
                        log::warn!("Калибровка отклонена: робот движется (rate={wheel_rate:.0} °/с)");
                    }
                }
                
                // Оценку дрейфа гироскопа обновляем только в покое: под
                // разгоном акселерометр принимает ускорение за наклон.
                let bias_adapt = wheel_rate.abs() < 200.0;

                let angle = filter.update(
                    data.accel_angle_deg,
                    gyro,
                    data.accel_magnitude_g,
                    dt,
                    bias_adapt,
                );

                // Автонастройка PID: релейный тест на фильтрованном угле.
                // Состояния Complete/Failed проверяются по state(), а не по
                // возвращаемому значению update() — в них update() отдает None.
                let relay_cmd = autotuner.update(angle, gyro, is_balancing, dt);
                match autotuner.state() {
                    autotune::TuneState::Oscillating => {
                        if let Some(cmd) = relay_cmd {
                            tele.update_imu(angle, gyro, data.accel_magnitude_g, data.accel_angle_deg);
                            engine.set_rates(cmd, cmd);
                            continue;
                        }
                    }
                    autotune::TuneState::Complete => {
                        if let Some(result) = autotuner.get_result() {
                            params.set("ka", result.ka);
                            params.set("kd", result.kd);
                            if result.ki > 0.0 {
                                params.set("ki", result.ki);
                            }
                            remote.set_autotune_result(
                                result.ka, result.kd, result.ki, result.ku, result.tu,
                            );
                            log::warn!(
                                "AUTOTUNE: применены Ka={:.0}, Kd={:.0}, Ki={:.1} (Ku={:.0}, Tu={:.3}с) — проверь робота!",
                                result.ka, result.kd, result.ki, result.ku, result.tu
                            );
                        }
                        remote.set_autotune_state(remote::AUTOTUNE_DONE);
                        autotuner.reset();
                    }
                    autotune::TuneState::Failed => {
                        log::error!("AUTOTUNE: не удалось (таймаут)");
                        remote.set_autotune_state(remote::AUTOTUNE_FAILED);
                        autotuner.reset();
                    }
                    _ => {}
                }

                let steps_now = engine.steps();
                let steps_delta = steps_now.wrapping_sub(last_steps);
                last_steps = steps_now;

                tele.update_imu(angle, gyro, data.accel_magnitude_g, data.accel_angle_deg);
                tele.set_gyro_bias(filter.bias());
                tele.set_trim(ctrl.effective_trim());

                let stop_requested = remote.take_disarm();
                let (drive_fwd, drive_trn) = remote.get_drive();

                if is_balancing {
                    // Проверка на убегание после арминга (первые 200 мс)
                    if let Some(arm_timer) = arm_fail_timer {
                        if arm_timer.elapsed() < Duration::from_millis(ARM_FAIL_CHECK_MS as u64) {
                            // Ещё в окне проверки
                            if angle.abs() > ARM_FAIL_MAX_ANGLE || gyro.abs() > ARM_FAIL_MAX_GYRO {
                                log::warn!("УБЕГАНИЕ при арминге: A={angle:.1}° G={gyro:.0} — СТОП");
                                tele.send_event(EV_ARM_FAIL, angle);
                                arm_fail_cooldown_until = Some(Instant::now() + Duration::from_millis(ARM_FAIL_COOLDOWN_MS as u64));
                                arm_fail_timer = None;
                                fall_detector.reset();
                                disarm(
                                    &mut is_balancing,
                                    &mut ctrl,
                                    &engine,
                                    &mut led,
                                    &tele,
                                    &mut arm_stable,
                                    &mut trim_sum,
                                )?;
                                continue;
                            }
                        } else {
                            // Окно проверки прошло успешно
                            arm_fail_timer = None;
                        }
                    }

                    // Ошибка слежения с учетом наклона хода (устраняет ложное падение)
                    let tracking_err = ctrl.tracking_error(angle);

                    // Улучшенный детектор падения: окно 5 измерений, тренд
                    // ошибки, асимметричные пороги вперёд/назад
                    let verdict = fall_detector.update(
                        tracking_err,
                        angle,
                        gyro,
                        dt,
                        tunables.fall_angle_deg,
                        tunables.fall_gyro_dps,
                    );

                    // Предупреждение о потере устойчивости — до падения
                    if verdict.warning {
                        log::warn!(
                            "ПОТЕРЯ УСТОЙЧИВОСТИ: e={tracking_err:.1}° тренд={:+.0}°/с — близко к падению",
                            verdict.trend_dps
                        );
                        tele.send_event(EV_STABILITY_WARN, tracking_err);
                    }

                    if verdict.falling || stop_requested {
                        if stop_requested {
                            log::warn!("СТОП по команде из UI");
                        } else {
                            log::warn!(
                                "УПАЛ e={tracking_err:.1}° A={angle:.1}° G={gyro:.0} тренд={:+.0}°/с — СТОП",
                                verdict.trend_dps
                            );
                        }
                        tele.send_event(EV_FALL, tracking_err);
                        tele.freeze_blackbox();

                        // Сбрасываем фильтр угла и дрейф нуля, чтобы не копить ошибку на полу
                        filter.reset(data.accel_angle_deg);
                        filter.reset_bias();

                        fall_detector.reset();
                        disarm(
                            &mut is_balancing,
                            &mut ctrl,
                            &engine,
                            &mut led,
                            &tele,
                            &mut arm_stable,
                            &mut trim_sum,
                        )?;
                    } else {
                        let (cmd_left, cmd_right) = ctrl.update(
                            angle,
                            gyro,
                            wheel_rate,
                            steps_delta,
                            drive_fwd,
                            drive_trn,
                            tunables.trim_offset,
                            tunables.max_drive_lean,
                            tunables.max_turn_rate,
                            tunables.drive_expo,
                            dt,
                        );
                        ctrl.adapt_trim(dt);

                        let limit = tunables.max_wheel_rps * stepper::STEPS_PER_REV;
                        engine.set_rates(
                            cmd_left.clamp(-limit, limit),
                            cmd_right.clamp(-limit, limit),
                        );

                        let (p, _lean, d, base_cmd) = ctrl.last_terms();
                        let i = ctrl.last_integral();
                        tele.update_control(base_cmd, wheel_rate, p, i, d, steps_now);
                        tele.set_state(STATE_BALANCING);

                        // Флаги состояния: бит 0 = баланс, бит 1 = препятствие,
                        // бит 2 = потеря устойчивости (предупреждение детектора)
                        let flags = 1u8 | if verdict.unstable { 4 } else { 0 };
                        
                        tele.record_blackbox(BlackBoxFrame {
                            t_ms: uptime_ms,
                            angle,
                            gyro,
                            cmd: base_cmd,
                            wheel_rate,
                            steps: steps_now,
                            dt_us: (raw_dt * 1_000_000.0) as u16,
                            pid_ka: tunables.ka_angle,
                            pid_kd: tunables.kd_gyro,
                            pid_ki: tunables.ki_angle,
                            flags,
                        });
                    }
                } else {
                    engine.stop();
                    tele.set_state(STATE_IDLE);
                    tele.update_control(0.0, wheel_rate, 0.0, 0.0, 0.0, steps_now);

                    if let Some(sign) = sign_check.update(
                        data.accel_angle_deg,
                        gyro,
                        data.accel_magnitude_g,
                        dt,
                    ) {
                        if sign < 0.0 {
                            imu.flip_gyro_sign();
                            filter.reset(data.accel_angle_deg);
                            log::warn!(
                                "Гироскоп был инвертирован относительно акселерометра — знак исправлен на {:+.0}",
                                imu.gyro_sign()
                            );
                        } else {
                            log::info!("Знаки IMU согласованы ✔");
                        }
                    }

                    // Реалистичный диапазон гравитации для удержания руками: 0.75g .. 1.25g
                    let still = angle.abs() < tunables.arm_max_angle
                        && gyro.abs() < ARM_MAX_GYRO_DPS
                        && wheel_rate.abs() < ARM_MAX_RATE
                        && (0.75..=1.25).contains(&data.accel_magnitude_g);

                    if still {
                        arm_stable = arm_stable.saturating_add(1);
                        trim_sum += angle;
                    } else {
                        arm_stable = 0;
                        trim_sum = 0.0;
                    }

                    // Если нажата кнопка в UI "ВКЛЮЧИТЬ БАЛАНС" — армим мгновенно при вертикальном положении
                    let remote_arm = remote.take_arm();
                    
                    // Проверка cooldown после неудачного арминга
                    let cooldown_active = arm_fail_cooldown_until
                        .map(|t| Instant::now() < t)
                        .unwrap_or(false);
                    
                    if remote_arm && !cooldown_active && angle.abs() < tunables.arm_max_angle {
                        arm_stable = ARM_STABLE_CYCLES;
                        trim_sum = angle * ARM_STABLE_CYCLES as f32;
                        arm_fail_timer = Some(Instant::now());
                    }

                    if arm_stable >= ARM_STABLE_CYCLES && arm_fail_timer.is_some() {
                        let held = trim_sum / ARM_STABLE_CYCLES as f32;
                        let trim = ctrl.arm(held);
                        filter.reset(angle);
                        filter.reset_bias();
                        last_steps = engine.steps();
                        engine.stop();
                        is_balancing = true;
                        arm_stable = 0;
                        trim_sum = 0.0;
                        led.set_high()?;
                        tele.set_state(STATE_BALANCING);
                        tele.send_event(EV_ARMED, trim);
                        tele.reset_blackbox();
                        fall_detector.reset();

                        log::info!("БАЛАНС ВКЛ trim={trim:.2}° — ОТПУСКАЙ РУКИ");
                    }

                    // Автокалибровка гироскопа: робот не в балансе, колёса стоят,
                    // корпус неподвижен (угол акселерометра стабилен, ~1g), а
                    // гироскоп упорно показывает скорость — это дрейф нуля.
                    // Перекалибровываем и привязываем фильтр к акселерометру,
                    // иначе дрейф наматывает угол (коррекция ограничена 8°/с
                    // и не пересиливает большой дрейф).
                    let body_still = (0.85..=1.15).contains(&data.accel_magnitude_g)
                        && wheel_rate.abs() < 50.0
                        && gyro.abs() < 30.0;
                    if body_still {
                        cal_accel_min = cal_accel_min.min(data.accel_angle_deg);
                        cal_accel_max = cal_accel_max.max(data.accel_angle_deg);
                        let accel_stable =
                            cal_accel_max - cal_accel_min <= AUTOCAL_ACCEL_SPREAD_DEG;

                        if accel_stable && gyro.abs() > AUTOCAL_DRIFT_DPS {
                            cal_stable = cal_stable.saturating_add(1);
                        } else {
                            cal_stable = 0;
                        }
                    } else {
                        cal_stable = 0;
                        cal_accel_min = f32::MAX;
                        cal_accel_max = f32::MIN;
                    }

                    let cal_cooldown_ok = cal_cooldown_until
                        .map(|t| now >= t)
                        .unwrap_or(true);

                    if cal_stable >= AUTOCAL_STABLE_CYCLES && cal_cooldown_ok {
                        log::info!(
                            "Автокалибровка: гироскоп {gyro:+.2}°/с в покое — калибрую ~1 с"
                        );
                        match imu.calibrate_with_result() {
                            Ok(spread) if spread <= CALIBRATION_MAX_SPREAD_DPS => {
                                filter.reset(data.accel_angle_deg);
                                filter.reset_bias();
                                log::info!(
                                    "Автокалибровка: offset={:+.3}°/с, разброс {spread:.2}°/с",
                                    imu.offset()
                                );
                            }
                            Ok(spread) => {
                                log::warn!(
                                    "Автокалибровка: шумно (разброс {spread:.2}°/с) — повторю позже"
                                );
                            }
                            Err(e) => {
                                log::error!("Автокалибровка: ошибка I2C {e:?}");
                            }
                        }

                        cal_cooldown_until = Some(Instant::now() + AUTOCAL_COOLDOWN);
                        cal_stable = 0;
                        cal_accel_min = f32::MAX;
                        cal_accel_max = f32::MIN;
                    }
                }

                let loop_us = loop_t0.elapsed().as_micros() as u32;
                tele.set_timing((raw_dt * 1_000_000.0) as u32, loop_us);
            }
            Err(err) => {
                imu_errors += 1;
                tele.set_imu_errors(imu_errors);
                log::warn!("Ошибка IMU ({imu_errors}/{MAX_IMU_ERRORS}): {err:?}");
                if imu_errors >= MAX_IMU_ERRORS {
                    tele.send_event(EV_IMU_DEAD, 0.0);
                    tele.freeze_blackbox();
                    fall_detector.reset();
                    disarm(
                        &mut is_balancing,
                        &mut ctrl,
                        &engine,
                        &mut led,
                        &tele,
                        &mut arm_stable,
                        &mut trim_sum,
                    )?;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn start_network(
    modem: esp_idf_svc::hal::modem::Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs_part: EspDefaultNvsPartition,
    storage: &Arc<Storage>,
    params: &Arc<ParamStore>,
    tele: &Arc<Telemetry>,
    status: &Arc<NetStatus>,
    remote: &Arc<RemoteState>,
) {
    if let Err(e) = net::wifi::spawn(
        modem,
        sysloop,
        nvs_part,
        storage.clone(),
        status.clone(),
    ) {
        log::error!("Wi-Fi не запущен: {e:#}");
        return;
    }

    if let Err(e) = net::telemetry_tx::spawn(tele.clone(), params.clone(), status.clone(), remote.clone()) {
        log::error!("Телеметрия по UDP не запущена: {e:#}");
    }

    if let Err(e) = net::command::spawn(net::command::Services {
        params: params.clone(),
        storage: storage.clone(),
        status: status.clone(),
        remote: remote.clone(),
        telemetry: tele.clone(),
    }) {
        log::error!("Приём команд не запущен: {e:#}");
    }

    if let Err(e) = net::http_api::spawn(params.clone(), tele.clone(), status.clone()) {
        log::error!("HTTP API не запущено: {e:#}");
    }

    log::info!("Сеть: телеметрия UDP {PORT_TELEMETRY}, команды TCP {PORT_COMMAND}, HTTP :{HTTP_PORT}");
    log::info!(
        "OTA: работаю из слота {}, обновление уйдёт в {}",
        net::ota::running_slot_label(),
        net::ota::update_slot_label()
    );
}

fn motor_direction_test(engine: &StepperEngine) {
    log::warn!("=== ТЕСТ МОТОРОВ: ПОДНИМИ РОБОТА, старт через 3 с ===");
    FreeRtos::delay_ms(3000);

    let rate = 800.0 * MOTOR_SIGN;
    log::info!("Фаза «+»: движение вперед");
    hold_rate(engine, rate, 2000);
    hold_rate(engine, 0.0, 500);
    log::info!("Фаза «−»: движение назад");
    hold_rate(engine, -rate, 2000);
    engine.stop();
    log::info!("=== ТЕСТ ЗАВЕРШЁН, ставь робота на пол ===");
    FreeRtos::delay_ms(1000);
}

fn hold_rate(engine: &StepperEngine, rate: f32, ms: u32) {
    let mut left = ms;
    while left > 0 {
        engine.set_rate(rate);
        let chunk = left.min(50);
        FreeRtos::delay_ms(chunk);
        left -= chunk;
    }
}

fn disarm(
    is_balancing: &mut bool,
    ctrl: &mut BalanceController,
    engine: &StepperEngine,
    led: &mut PinDriver<'_, Output>,
    tele: &Telemetry,
    arm_stable: &mut u32,
    trim_sum: &mut f32,
) -> anyhow::Result<()> {
    *is_balancing = false;
    ctrl.disarm();
    engine.stop();
    led.set_low()?;
    *arm_stable = 0;
    *trim_sum = 0.0;
    tele.set_state(STATE_IDLE);
    Ok(())
}

fn wait_imu_angle(imu: &mut Mpu6050<'_>) -> anyhow::Result<f32> {
    let mut attempts = 0u32;
    loop {
        match imu.read_angle_and_rate() {
            Ok((angle, _)) => return Ok(angle),
            Err(e) => {
                attempts += 1;
                if attempts > 20 {
                    anyhow::bail!("IMU не отвечает при старте: {e:?}");
                }
                log::warn!("IMU старт не удался ({e:?}), попытка {attempts}");
                FreeRtos::delay_ms(100);
            }
        }
    }
}