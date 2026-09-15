//! Каскадный контроллер балансировщика с поддержкой скоростного хода и трима угла.

use crate::stepper::MAX_STEP_RATE;

pub const MOTOR_SIGN: f32 = 1.0;

const TRIM_ADAPT_RATE: f32 = 0.45;
const TRIM_MAX_DRIFT_DEG: f32 = 6.0;
const SOFT_GAIN_START: f32 = 0.12;
const SOFT_RAMP_PER_SEC: f32 = 2.0;
const SPEED_BRAKE_RATE: f32 = 4.0;
const POSITION_LIMIT_STEPS: f32 = 16_000.0;

#[derive(Clone, Copy)]
pub struct Gains {
    pub ka_angle: f32,
    pub kd_gyro: f32,
    pub kp_pos: f32,
    pub kd_vel: f32,
    pub max_lean_deg: f32,
}

impl Default for Gains {
    fn default() -> Self {
        Self {
            ka_angle: 3800.0,
            kd_gyro: 600.0,
            kp_pos: 0.0005,
            kd_vel: 0.00125,
            max_lean_deg: 10.0,
        }
    }
}

pub struct BalanceController {
    gains: Gains,
    angle_trim: f32,
    trim_base: f32,
    last_trim_offset: f32,
    lean_from_position: f32,
    position: f32,
    speed: f32,
    soft_gain: f32,
    soft_ramp_per_sec: f32,
    last_p: f32,
    last_d: f32,
    last_lean: f32,
    last_cmd: f32,
    learned_trim: Option<f32>,
}

impl BalanceController {
    pub fn new(gains: Gains) -> Self {
        Self {
            gains,
            angle_trim: 0.0,
            trim_base: 0.0,
            last_trim_offset: 0.0,
            lean_from_position: 0.0,
            position: 0.0,
            speed: 0.0,
            soft_gain: 0.0,
            soft_ramp_per_sec: SOFT_RAMP_PER_SEC,
            last_p: 0.0,
            last_d: 0.0,
            last_lean: 0.0,
            last_cmd: 0.0,
            learned_trim: None,
        }
    }

    pub fn arm(&mut self, held_angle: f32) -> f32 {
        let angle_trim = match self.learned_trim {
            Some(learned) if (learned - held_angle).abs() < 6.0 => learned,
            _ => held_angle,
        };

        self.angle_trim = angle_trim;
        self.trim_base = angle_trim;
        self.last_trim_offset = 0.0;
        self.lean_from_position = 0.0;
        self.position = 0.0;
        self.speed = 0.0;
        self.soft_gain = SOFT_GAIN_START;
        self.soft_ramp_per_sec = SOFT_RAMP_PER_SEC;
        self.last_p = 0.0;
        self.last_d = 0.0;
        self.last_lean = 0.0;
        self.last_cmd = 0.0;
        angle_trim
    }

    pub fn disarm(&mut self) {
        if self.soft_gain >= 0.95 {
            self.learned_trim = Some(self.angle_trim);
        }
        self.soft_gain = 0.0;
        self.position = 0.0;
        self.speed = 0.0;
        self.last_cmd = 0.0;
    }

    /// Эффективная точка равновесия с учетом пользовательского смещения (трима).
    pub fn effective_trim(&self) -> f32 {
        self.angle_trim + self.last_trim_offset
    }

    pub fn set_gains(&mut self, gains: Gains) {
        self.gains = gains;
    }

    pub fn last_terms(&self) -> (f32, f32, f32, f32) {
        (self.last_p, self.last_lean, self.last_d, self.last_cmd)
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
        dt: f32,
    ) -> (f32, f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return (self.last_cmd, self.last_cmd);
        }

        self.last_trim_offset = trim_offset;
        self.soft_gain = (self.soft_gain + self.soft_ramp_per_sec * dt).min(1.0);

        let wheel_rate_logical = wheel_rate * MOTOR_SIGN;
        let is_driving = drive_forward.abs() > 0.02;

        let g = &self.gains;

        let lean = if is_driving {
            self.position = 0.0;
            self.lean_from_position = 0.0;

            let max_target_speed = MAX_STEP_RATE * 0.70;
            let target_velocity = -drive_forward * max_target_speed;

            let speed_error = wheel_rate_logical - target_velocity;
            let lean_speed_feedback = (g.kd_vel * speed_error).clamp(-max_drive_lean_deg, max_drive_lean_deg);
            let lean_feedforward = drive_forward * max_drive_lean_deg;

            (lean_speed_feedback + lean_feedforward).clamp(-g.max_lean_deg, g.max_lean_deg)
        } else {
            self.position += steps_delta as f32 * MOTOR_SIGN;
            self.position = self.position.clamp(-POSITION_LIMIT_STEPS, POSITION_LIMIT_STEPS);

            self.lean_from_position = -g.kp_pos * self.position;
            (self.lean_from_position - g.kd_vel * wheel_rate_logical).clamp(-g.max_lean_deg, g.max_lean_deg)
        };

        // Эффективный целевой угол включает базовый трим + пользовательское смещение
        let target_angle = self.angle_trim + trim_offset;
        let err = (angle_deg - target_angle) - lean;

        let p = g.ka_angle * err;
        let d = g.kd_gyro * gyro_dps;
        let accel = (p + d) * self.soft_gain;

        let next = self.speed + accel * dt;
        let saturated = next.abs() > MAX_STEP_RATE
            && next.signum() == self.speed.signum()
            && accel.signum() == self.speed.signum();
        if !saturated {
            self.speed = next.clamp(-MAX_STEP_RATE, MAX_STEP_RATE);
        } else {
            self.speed = self.speed.clamp(-MAX_STEP_RATE, MAX_STEP_RATE);
        }

        if !is_driving && self.speed * err < 0.0 && self.speed.abs() > 800.0 {
            let brake = (1.0 - SPEED_BRAKE_RATE * dt).clamp(0.0, 1.0);
            self.speed *= brake;
        }

        self.last_lean = lean;
        self.last_p = p;
        self.last_d = d;

        let base_cmd = self.speed * MOTOR_SIGN;
        self.last_cmd = base_cmd;

        let turn_offset = drive_turn * max_turn_rate;
        let left_cmd = base_cmd - turn_offset;
        let right_cmd = base_cmd + turn_offset;

        (left_cmd, right_cmd)
    }

    pub fn adapt_trim(&mut self, dt: f32) {
        if self.soft_gain < 0.95 || self.position.abs() > 2000.0 {
            return;
        }
        self.angle_trim += self.lean_from_position * TRIM_ADAPT_RATE * dt;
        self.angle_trim = self.angle_trim.clamp(
            self.trim_base - TRIM_MAX_DRIFT_DEG,
            self.trim_base + TRIM_MAX_DRIFT_DEG,
        );
    }
}