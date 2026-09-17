//! Каскадный контроллер балансировщика: контур скорости хода поверх контура баланса.
//!
//! Физика связывает знаки жёстко: чтобы колёса ускорились в сторону D,
//! корпус должен наклониться в сторону D (равновесный угол = trim + lean,
//! колёса «догоняют» падение). Поэтому во всех ветках наклон строится
//! по недобору скорости: lean = kv·(цель − факт). Так уже работали
//! торможение и удержание позиции; контур хода теперь устроен так же.

use crate::stepper::MAX_STEP_RATE;

pub const MOTOR_SIGN: f32 = 1.0;

const TRIM_ADAPT_RATE: f32 = 0.45;
const TRIM_MAX_DRIFT_DEG: f32 = 6.0;
const SOFT_GAIN_START: f32 = 0.12;
const SOFT_RAMP_PER_SEC: f32 = 2.0;
const SPEED_BRAKE_RATE: f32 = 4.0;
const POSITION_LIMIT_STEPS: f32 = 16_000.0;

// Безопасная крейсерская скорость (шаг/с): 1.5 об/с ≈ 0.4 м/с.
// Оставляет 50% запаса мотора на удержание равновесия.
const MAX_CRUISE_SPEED: f32 = 4800.0;

/// Скорость нарастания целевой скорости хода (шаг/с²). Разгон до крейсера
/// за ~0.8 с; при этом контр скорости выходит на предельный наклон хода.
const TARGET_SLEW_RATE: f32 = 6000.0;

/// Скорость нарастания ускорения для S-образного профиля (шаг/с³).
/// Определяет плавность перехода от разгона к постоянной скорости.
/// Меньше значение = более плавный S-образный профиль.
const S_CURVE_JERK: f32 = 15000.0;

/// Коэффициент формы S-кривой. 1.0 = линейный профиль, >1.0 = более выраженный S.
const S_CURVE_SHAPE: f32 = 1.5;

/// Недобор скорости до цели (шаг/с), после которого включается дожим
/// препятствий. Выше типичного недобора при разгоне (~3000 шаг/с при
/// полном темпе нарастания), чтобы дожим срабатывал только на реальном
/// упоре в препятствие, а не на каждом старте.
const OBSTACLE_ENGAGE_ERROR: f32 = 3500.0;

// Скорость накопления дополнительного наклона при упоре в препятствие (град/сек)
const OBSTACLE_BOOST_RATE: f32 = 6.0;
const MAX_BOOST_LEAN: f32 = 4.0;

#[derive(Clone, Copy)]
pub struct Gains {
    pub ka_angle: f32,
    pub kd_gyro: f32,
    pub ki_angle: f32,
    pub kp_pos: f32,
    pub kd_vel: f32,
    pub max_lean_deg: f32,
}

impl Default for Gains {
    fn default() -> Self {
        Self {
            ka_angle: 3800.0,
            kd_gyro: 600.0,
            ki_angle: 0.0,
            kp_pos: 0.0005,
            kd_vel: 0.0010,
            max_lean_deg: 9.0,
        }
    }
}

pub struct BalanceController {
    gains: Gains,
    angle_trim: f32,
    trim_base: f32,
    last_trim_offset: f32,
    lean_from_position: f32,
    obstacle_boost: f32,
    /// Текущая цель контура скорости (шаг/с), наращивается плавно.
    target_velocity: f32,
    /// Целевое ускорение для S-профиля (шаг/с²)
    target_accel: f32,
    position: f32,
    speed: f32,
    soft_gain: f32,
    soft_ramp_per_sec: f32,
    integral_error: f32,
    last_p: f32,
    last_d: f32,
    last_lean: f32,
    last_cmd: f32,
}

impl BalanceController {
    pub fn new(gains: Gains) -> Self {
        Self {
            gains,
            angle_trim: 0.0,
            trim_base: 0.0,
            last_trim_offset: 0.0,
            lean_from_position: 0.0,
            obstacle_boost: 0.0,
            target_velocity: 0.0,
            target_accel: 0.0,
            position: 0.0,
            speed: 0.0,
            soft_gain: 0.0,
            soft_ramp_per_sec: SOFT_RAMP_PER_SEC,
            integral_error: 0.0,
            last_p: 0.0,
            last_d: 0.0,
            last_lean: 0.0,
            last_cmd: 0.0,
        }
    }

    /// Точка равновесия всегда берётся из текущего удерживаемого угла:
    /// как держишь — так и встанет. Пока робот стоит, `adapt_trim`
    /// сам доучит её под реальную поверхность.
    pub fn arm(&mut self, held_angle: f32) -> f32 {
        self.angle_trim = held_angle;
        self.trim_base = held_angle;
        self.last_trim_offset = 0.0;
        self.lean_from_position = 0.0;
        self.obstacle_boost = 0.0;
        self.target_velocity = 0.0;
        self.target_accel = 0.0;
        self.position = 0.0;
        self.speed = 0.0;
        self.soft_gain = SOFT_GAIN_START;
        self.soft_ramp_per_sec = SOFT_RAMP_PER_SEC;
        self.integral_error = 0.0;
        self.last_p = 0.0;
        self.last_d = 0.0;
        self.last_lean = 0.0;
        self.last_cmd = 0.0;
        held_angle
    }

    pub fn disarm(&mut self) {
        self.soft_gain = 0.0;
        self.target_velocity = 0.0;
        self.target_accel = 0.0;
        self.position = 0.0;
        self.speed = 0.0;
        self.last_cmd = 0.0;
        self.last_lean = 0.0;
        self.obstacle_boost = 0.0;
        self.integral_error = 0.0;
    }

    pub fn effective_trim(&self) -> f32 {
        self.angle_trim + self.last_trim_offset
    }

    pub fn tracking_error(&self, angle_deg: f32) -> f32 {
        (angle_deg - self.effective_trim()) - self.last_lean
    }

    pub fn set_gains(&mut self, gains: Gains) {
        self.gains = gains;
    }

    pub fn last_terms(&self) -> (f32, f32, f32, f32) {
        (self.last_p, self.last_lean, self.last_d, self.last_cmd)
    }

    pub fn last_integral(&self) -> f32 {
        self.integral_error
    }

    pub fn update(
        &mut self,
        angle_deg: f32,
        gyro_dps: f32,
        wheel_rate: f32,
        steps_delta: i32,
        drive_forward: f32,
        drive_turn: f32,
        trim_offset: f32,
        max_drive_lean_deg: f32,
        max_turn_rate: f32,
        drive_expo: f32,
        dt: f32,
    ) -> (f32, f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return (self.last_cmd, self.last_cmd);
        }

        self.last_trim_offset = trim_offset;
        self.soft_gain = (self.soft_gain + self.soft_ramp_per_sec * dt).min(1.0);

        let wheel_rate_logical = wheel_rate * MOTOR_SIGN;
        let is_driving = drive_forward.abs() > 0.03;

        // Экспоненциальная кривая газа: sign(x) * |x|^expo
        let drive_forward = if is_driving && drive_expo > 0.0 {
            let expo = drive_expo.clamp(0.0, 0.8);
            let sign = drive_forward.signum();
            let abs = drive_forward.abs();
            sign * abs.powf(1.0 - expo)
        } else {
            drive_forward
        };

        let g = &self.gains;

        // === Единый контур скорости с S-образным профилем ===
        //
        // 1. Цель: вперёд = положительная скорость колёс (та же конвенция,
        //    что в motor_direction_test: фаза «+» = движение вперёд).
        let commanded_velocity = if is_driving {
            drive_forward * MAX_CRUISE_SPEED
        } else {
            0.0
        };

        // 2. S-образный профиль разгона/торможения через управление ускорением.
        //    Используем сигмоидальную функцию для плавного изменения ускорения.
        let velocity_error = commanded_velocity - self.target_velocity;
        let error_abs = velocity_error.abs();
        
        // Целевое ускорение с S-профилем: чем больше ошибка, тем больше ускорение,
        // но с плавным насыщением по сигмоиде.
        let target_accel_mag = if error_abs < 10.0 {
            // Малая ошибка: линейный режим
            error_abs * TARGET_SLEW_RATE / 1000.0
        } else {
            // Большая ошибка: S-образное насыщение
            let normalized_error = error_abs / 1000.0;
            let sigmoid = (S_CURVE_SHAPE * normalized_error).tanh();
            sigmoid * TARGET_SLEW_RATE
        };
        
        // Плавное изменение ускорения (jerk limit)
        let accel_dt = (target_accel_mag - self.target_accel.abs()) * S_CURVE_JERK * dt;
        self.target_accel += accel_dt.signum() * accel_dt.abs().min(S_CURVE_JERK * dt);
        self.target_accel = self.target_accel.clamp(-TARGET_SLEW_RATE, TARGET_SLEW_RATE);
        
        // Интегрируем ускорение в скорость
        let max_velocity_step = self.target_accel * dt;
        self.target_velocity += velocity_error.signum() * max_velocity_step.min(velocity_error.abs());

        // 3. Наклон в сторону недобора скорости: колёса медленнее цели —
        //    наклоняемся вперёд к цели, колёса быстрее — отклоняемся назад.
        //    (Прежняя формула kv·(факт − цель) была положительной обратной
        //    связью: робот разгонялся прочь от цели и не реагировал на реверс.)
        let speed_error = self.target_velocity - wheel_rate_logical;
        let base_lean = (g.kd_vel * speed_error).clamp(-max_drive_lean_deg, max_drive_lean_deg);

        // 4. Интегратор препятствий: колёса упёрлись и не догоняют цель —
        //    дожимаем корпус в ту же сторону, куда хотим ехать.
        if is_driving && speed_error.abs() > OBSTACLE_ENGAGE_ERROR {
            let boost_delta = speed_error.signum() * OBSTACLE_BOOST_RATE * dt;
            self.obstacle_boost = (self.obstacle_boost + boost_delta).clamp(-MAX_BOOST_LEAN, MAX_BOOST_LEAN);
        } else {
            // Плавный возврат интегратора к нулю, когда робот едет свободно
            self.obstacle_boost *= (1.0 - 2.0 * dt).clamp(0.0, 1.0);
        }

        let mut lean = base_lean + self.obstacle_boost;

        // 5. Удержание точки стоянки: включается, когда цель нулевая
        //    и колёса почти остановились.
        if !is_driving {
            if wheel_rate_logical.abs() > 300.0 {
                // ещё катимся после отпущенного газа — точку не запоминаем
                self.position = 0.0;
                self.lean_from_position = 0.0;
            } else {
                self.position += steps_delta as f32 * MOTOR_SIGN;
                self.position = self.position.clamp(-POSITION_LIMIT_STEPS, POSITION_LIMIT_STEPS);

                self.lean_from_position = -g.kp_pos * self.position;
                lean += self.lean_from_position;
            }
        } else {
            self.position = 0.0;
            self.lean_from_position = 0.0;
        }

        let lean = lean.clamp(-g.max_lean_deg, g.max_lean_deg);

        let target_angle = self.angle_trim + trim_offset;
        let err = (angle_deg - target_angle) - lean;

        self.integral_error += err * dt;
        self.integral_error = self.integral_error.clamp(-10.0, 10.0);

        let p = g.ka_angle * err;
        let i = g.ki_angle * self.integral_error;
        let d = g.kd_gyro * gyro_dps;
        let accel = (p + i + d) * self.soft_gain;

        let next = self.speed + accel * dt;
        let saturated = next.abs() > MAX_STEP_RATE
            && next.signum() == self.speed.signum()
            && accel.signum() == self.speed.signum();

        if !saturated {
            self.speed = next.clamp(-MAX_STEP_RATE, MAX_STEP_RATE);
        } else {
            self.speed = self.speed.clamp(-MAX_STEP_RATE, MAX_STEP_RATE);
        }

        // Защитный тормоз от столкновения со стеной (только при отпущенном газе)
        if !is_driving && self.speed * err < 0.0 && self.speed.abs() > 800.0 {
            let brake = (1.0 - SPEED_BRAKE_RATE * dt).clamp(0.0, 1.0);
            self.speed *= brake;
        }

        self.last_lean = lean;
        self.last_p = p;
        self.last_d = d;

        let base_cmd = self.speed * MOTOR_SIGN;
        self.last_cmd = base_cmd;

        // Поворот вокруг вертикальной оси
        let turn_offset = drive_turn * max_turn_rate;
        let left_cmd = (base_cmd - turn_offset).clamp(-MAX_STEP_RATE, MAX_STEP_RATE);
        let right_cmd = (base_cmd + turn_offset).clamp(-MAX_STEP_RATE, MAX_STEP_RATE);

        (left_cmd, right_cmd)
    }

    pub fn adapt_trim(&mut self, dt: f32) {
        // Порог сползания, при котором ещё доучиваем точку равновесия:
        // 6000 шагов ≈ два оборота колеса — этого хватает, чтобы перевыучить
        // трим на неровном полу или пологом уклоне, а не только на идеальном.
        if self.soft_gain < 0.95 || self.position.abs() > 6_000.0 {
            return;
        }
        self.angle_trim += self.lean_from_position * TRIM_ADAPT_RATE * dt;
        self.angle_trim = self.angle_trim.clamp(
            self.trim_base - TRIM_MAX_DRIFT_DEG,
            self.trim_base + TRIM_MAX_DRIFT_DEG,
        );
    }
}