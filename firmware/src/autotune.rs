//! Автонастройка PID-коэффициентов методом релейной обратной связи.
//!
//! Процедура: робот выключен из баланса и СВОБОДНО висит на руке
//! (колёса не касаются пола, корпус не зафиксирован — рука шарнир).
//! Реле подаёт ±ускорение колёс по знаку угла — корпус раскачивается.
//! По периоду и амплитуде автоколебаний вычисляются критический
//! коэффициент Ku и период Tu, затем коэффициенты Зиглера-Николса:
//!   Ka = 0.6·Ku
//!   Kd = 0.6·Ku·Tu/8   (классический ЗН: Kp·Td, Td = Tu/8)
//!   Ki = 0.12·Ku/Tu    (по умолчанию выключен)

use std::time::Instant;

use crate::stepper::MAX_STEP_RATE;

/// Гистерезис знака угла (°): защита от шума возле нуля.
const HYSTERESIS_DEG: f32 = 0.5;
/// Сколько циклов (200 Гц = 100 мс) робот должен быть неподвижен перед стартом.
const STILL_CYCLES: u32 = 20;
/// Таймауты фаз (с).
const WAIT_TIMEOUT_S: f32 = 5.0;
const OSC_TIMEOUT_S: f32 = 15.0;
/// Сколько полных периодов собрать до расчёта.
const PERIODS_NEEDED: usize = 3;
/// Границы корректного периода (с).
const MIN_PERIOD_S: f32 = 0.2;
const MAX_PERIOD_S: f32 = 5.0;
/// Минимальная амплитуда для расчёта Ku (°) — защита от деления на шум.
const MIN_AMPLITUDE_DEG: f32 = 0.5;

/// Состояния автомата автонастройки
#[derive(Clone, Copy, PartialEq)]
pub enum TuneState {
    Idle,
    Waiting,      // Ожидание неподвижности
    Oscillating,  // Раскачка реле
    Measuring,    // Расчёт коэффициентов
    Complete,     // Завершено
    Failed,       // Ошибка
}

/// Результат автонастройки
pub struct TuneResult {
    pub ka: f32,
    pub kd: f32,
    pub ki: f32,
    pub ku: f32, // Критический коэффициент
    pub tu: f32, // Период колебаний (с)
}

/// Релейный тестер для автонастройки
pub struct RelayTuner {
    state: TuneState,
    /// Амплитуда реле — ускорение колёс (шаг/с²), в тех же единицах,
    /// что и выход балансного контура (ka·err).
    relay_accel: f32,
    /// Интегратор реле: текущая скорость колёс (шаг/с).
    relay_speed: f32,
    /// Последний уверенный знак угла: true = наклон вперёд.
    sign_pos: Option<bool>,
    /// Амплитуда текущего полупериода (°).
    peak: f32,
    peaks: Vec<f32>,
    periods: Vec<f32>,
    period_start: Option<Instant>,
    timer: Instant,
    still_cycles: u32,
    use_integral: bool,
}

impl RelayTuner {
    pub fn new() -> Self {
        Self {
            state: TuneState::Idle,
            relay_accel: 2500.0,
            relay_speed: 0.0,
            sign_pos: None,
            peak: 0.0,
            peaks: Vec::with_capacity(PERIODS_NEEDED + 1),
            periods: Vec::with_capacity(PERIODS_NEEDED + 1),
            period_start: None,
            timer: Instant::now(),
            still_cycles: 0,
            use_integral: false,
        }
    }

    /// Запуск автонастройки
    pub fn start(&mut self) {
        self.state = TuneState::Waiting;
        self.timer = Instant::now();
        self.relay_speed = 0.0;
        self.sign_pos = None;
        self.peak = 0.0;
        self.peaks.clear();
        self.periods.clear();
        self.period_start = None;
        self.still_cycles = 0;
    }

    /// Сброс в idle
    pub fn reset(&mut self) {
        self.state = TuneState::Idle;
    }

    /// Основной цикл: вызывается 200 Гц.
    /// Возвращает команду скорости моторам (шаг/с) или None для обычного режима.
    pub fn update(&mut self, angle: f32, gyro: f32, _is_balancing: bool, dt: f32) -> Option<f32> {
        match self.state {
            TuneState::Idle => None,

            TuneState::Waiting => {
                // Ждём неподвижности: угол < 2°, гироскоп < 5°/с, 100 мс подряд
                if angle.abs() < 2.0 && gyro.abs() < 5.0 {
                    self.still_cycles = self.still_cycles.saturating_add(1);
                } else {
                    self.still_cycles = 0;
                }

                if self.still_cycles >= STILL_CYCLES {
                    self.state = TuneState::Oscillating;
                    self.timer = Instant::now();
                    Some(0.0)
                } else if self.timer.elapsed().as_secs_f32() > WAIT_TIMEOUT_S {
                    log::warn!("AUTOTUNE: робот не успокоился за {WAIT_TIMEOUT_S:.0} с");
                    self.state = TuneState::Failed;
                    None
                } else {
                    Some(0.0)
                }
            }

            TuneState::Oscillating => {
                // Знак угла с гистерезисом: внутри ±0.5° знак не определён
                let sign = if angle > HYSTERESIS_DEG {
                    Some(true)
                } else if angle < -HYSTERESIS_DEG {
                    Some(false)
                } else {
                    None
                };

                // Полный период: пересечение снизу вверх (от «назад» к «вперёд»)
                if sign == Some(true) && self.sign_pos == Some(false) {
                    if let Some(start) = self.period_start {
                        let period = start.elapsed().as_secs_f32();
                        if (MIN_PERIOD_S..=MAX_PERIOD_S).contains(&period) {
                            self.periods.push(period);
                            self.peaks.push(self.peak);
                            log::info!(
                                "AUTOTUNE: период {:.3} с, амплитуда {:.2}° ({}/{PERIODS_NEEDED})",
                                period,
                                self.peak,
                                self.periods.len()
                            );
                        }
                    }
                    self.period_start = Some(Instant::now());
                    self.peak = 0.0;
                }

                // Любая смена знака — переворот реле
                if sign.is_some() && sign != self.sign_pos {
                    self.sign_pos = sign;
                }

                self.peak = self.peak.max(angle.abs());

                // Реле: ускорение по знаку угла (знак как у P-члена контроллера),
                // интегрируется в скорость колёс — как это делает балансный контур.
                let dir = if self.sign_pos == Some(false) { -1.0 } else { 1.0 };
                self.relay_speed += dir * self.relay_accel * dt;
                self.relay_speed = self.relay_speed.clamp(-MAX_STEP_RATE, MAX_STEP_RATE);

                if self.periods.len() >= PERIODS_NEEDED {
                    self.state = TuneState::Measuring;
                    return Some(self.relay_speed);
                }

                if self.timer.elapsed().as_secs_f32() > OSC_TIMEOUT_S {
                    log::warn!(
                        "AUTOTUNE: колебания не возникли за {OSC_TIMEOUT_S:.0} с \
                         (угол не пересекает ноль — корпус зафиксирован?)"
                    );
                    self.state = TuneState::Failed;
                    return None;
                }

                Some(self.relay_speed)
            }

            TuneState::Measuring => {
                if self.compute_result().is_none() {
                    self.state = TuneState::Failed;
                    return None;
                }
                self.state = TuneState::Complete;
                None
            }

            TuneState::Complete => None,
            TuneState::Failed => None,
        }
    }

    fn compute_result(&self) -> Option<TuneResult> {
        if self.periods.is_empty() || self.peaks.is_empty() {
            return None;
        }

        let tu = self.periods.iter().sum::<f32>() / self.periods.len() as f32;
        let amplitude = (self.peaks.iter().sum::<f32>() / self.peaks.len() as f32)
            .max(MIN_AMPLITUDE_DEG);

        // Критический коэффициент: Ku = 4·d / (π·a)
        let ku = 4.0 * self.relay_accel / (std::f32::consts::PI * amplitude);

        // Зиглер-Николс: Ka = 0.6·Ku, Kd = Ka·Tu/8, Ki = 1.2·Ku/Tu (ослаблен ×10)
        let ka = 0.6 * ku;
        let kd = ka * tu / 8.0;
        let ki = if self.use_integral { 0.12 * ku / tu } else { 0.0 };

        log::info!(
            "AUTOTUNE: Tu={tu:.3} с, амплитуда={amplitude:.2}° → Ku={ku:.0}",
        );
        log::info!("  → Ka={ka:.0}, Kd={kd:.0}, Ki={ki:.1}");

        Some(TuneResult { ka, kd, ki, ku, tu })
    }

    /// Получить результат автонастройки
    pub fn get_result(&self) -> Option<TuneResult> {
        if self.state != TuneState::Complete {
            return None;
        }
        self.compute_result()
    }

    pub fn state(&self) -> TuneState {
        self.state
    }

    #[allow(dead_code)]
    pub fn set_relay_amplitude(&mut self, amp: f32) {
        self.relay_accel = amp.clamp(500.0, 10_000.0);
    }

    #[allow(dead_code)]
    pub fn set_use_integral(&mut self, use_i: bool) {
        self.use_integral = use_i;
    }
}

impl Default for RelayTuner {
    fn default() -> Self {
        Self::new()
    }
}
