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

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{Output, PinDriver};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use esp_idf_svc::hal::units::FromValueType;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use esp_idf_svc::sys::esp_task_wdt_add;

use std::sync::Arc;
use std::time::{Duration, Instant};

use control::{BalanceController, MOTOR_SIGN};
use filter::ComplementaryFilter;
use mpu6050::Mpu6050;
use net::{NetStatus, PORT_COMMAND, PORT_TELEMETRY};
use params::{ParamStore, Params};
use remote::RemoteState;
use signcheck::GyroSignCheck;
use stepper::{StepperEngine, StepperMotor, MAX_STEP_RATE, MICROSTEPS};
use storage::Storage;
use telemetry::{BlackBoxFrame, Telemetry, EV_ARMED, EV_FALL, EV_IMU_DEAD, STATE_BALANCING, STATE_IDLE};

const TELEMETRY_PERIOD: Duration = Duration::from_millis(200);

// Реалистичные пороги для удержания робота руками человека
const ARM_MAX_GYRO_DPS: f32 = 16.0;  // Тремор рук обычно 8-12°/с
const ARM_MAX_RATE: f32 = 60.0;
const ARM_STABLE_CYCLES: u32 = 60;   // 60 × 5 мс = 300 мс неподвижности (вместо 500 мс)
const MAX_IMU_ERRORS: u32 = 3;
const OTA_CONFIRM_AFTER: Duration = Duration::from_secs(30);
const MOTOR_TEST_ON_BOOT: bool = false;

const CONTROL_PERIOD_TICKS: u32 = 5;

/// Период сброса watchdog в главном цикле (мс)
const WDT_FEED_PERIOD_MS: u32 = 500;

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

    // Watchdog: перезагрузка при зависании > 3 сек
    unsafe {
        esp_task_wdt_add(std::ptr::null_mut());
    }
    log::info!("Watchdog: инициализирован (timeout 3с)");

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
    let mut sign_check = GyroSignCheck::new();
    let mut led = PinDriver::output(peripherals.pins.gpio2)?;
    led.set_low()?;

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
    let mut resync_stable = 0u32;
    let mut arm_requested_until: Option<Instant> = None;
    let mut last_steps = engine.steps();

    let boot = Instant::now();
    let mut ota_confirmed = false;
    let mut last_imu_time = Instant::now();
    let mut last_wdt_feed = Instant::now();

    let mut last_wake = unsafe { esp_idf_svc::sys::xTaskGetTickCount() };

    loop {
        unsafe {
            esp_idf_svc::sys::xTaskDelayUntil(&mut last_wake, CONTROL_PERIOD_TICKS);
        }

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

        // Watchdog: кормим каждые 500 мс
        if last_wdt_feed.elapsed() > Duration::from_millis(500) {
            unsafe {
                esp_idf_svc::sys::esp_task_wdt_reset();
            }
            last_wdt_feed = Instant::now();
        }

        match imu.read() {
            Ok(data) => {
                imu_errors = 0;
                tele.set_imu_errors(0);

                let gyro = data.gyro_rate_dps;
                let wheel_rate = engine.actual_rate();
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
                let steps_now = engine.steps();
                let steps_delta = steps_now.wrapping_sub(last_steps);
                last_steps = steps_now;

                tele.update_imu(angle, gyro, data.accel_magnitude_g, data.accel_angle_deg);
                tele.set_gyro_bias(filter.bias());
                tele.set_trim(ctrl.effective_trim());

                let stop_requested = remote.take_disarm();
                let (drive_fwd, drive_trn) = remote.get_drive();

                if is_balancing {
                    // Ошибка слежения с учетом наклона хода (устраняет ложное падение)
                    let tracking_err = ctrl.tracking_error(angle);
                    
                    let falling = tracking_err.abs() > tunables.fall_angle_deg
                        || angle.abs() > 35.0
                        || (tracking_err.abs() > 12.0 && gyro.abs() > tunables.fall_gyro_dps);

                    if falling || stop_requested {
                        if stop_requested {
                            log::warn!("СТОП по команде из UI");
                        } else {
                            log::warn!("УПАЛ e={tracking_err:.1}° A={angle:.1}° G={gyro:.0} — СТОП");
                        }
                        tele.send_event(EV_FALL, tracking_err);
                        tele.freeze_blackbox();

                        // Сбрасываем фильтр угла и дрейф нуля, чтобы не копить ошибку на полу
                        filter.reset(data.accel_angle_deg);
                        filter.reset_bias();

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
                            dt,
                        );
                        ctrl.adapt_trim(dt);

                        let limit = tunables.max_wheel_rps * stepper::STEPS_PER_REV;
                        engine.set_rates(
                            cmd_left.clamp(-limit, limit),
                            cmd_right.clamp(-limit, limit),
                        );

                        let (p, lean, d, base_cmd) = ctrl.last_terms();
                        let i = ctrl.last_integral();
                        tele.update_control(base_cmd, wheel_rate, p, i, d, steps_now);
                        tele.set_state(STATE_BALANCING);

                        tele.record_blackbox(BlackBoxFrame {
                            t_ms: uptime_ms,
                            angle,
                            gyro,
                            cmd: base_cmd,
                            wheel_rate,
                            steps: steps_now,
                            dt_us: (raw_dt * 1_000_000.0) as u16,
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
                    if remote_arm && angle.abs() < tunables.arm_max_angle {
                        arm_stable = ARM_STABLE_CYCLES;
                        trim_sum = angle * ARM_STABLE_CYCLES as f32;
                    }

                    if arm_stable >= ARM_STABLE_CYCLES {
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

                        log::info!("БАЛАНС ВКЛ trim={trim:.2}° — ОТПУСКАЙ РУКИ");
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

    log::info!("Сеть: телеметрия UDP {PORT_TELEMETRY}, команды TCP {PORT_COMMAND}");
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