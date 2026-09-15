//! Оставлен как простой переиспользуемый PID (не используется балансиром
//! напрямую — см. `control::BalanceController`).

pub struct PidController {
    kp: f32,
    ki: f32,
    kd: f32,
    integral_error: f32,
    integral_limit: f32,
    output_limit: f32,
    initialized: bool,
}

impl PidController {
    pub fn new(kp: f32, ki: f32, kd: f32, integral_limit: f32, output_limit: f32) -> Self {
        Self {
            kp,
            ki,
            kd,
            integral_error: 0.0,
            integral_limit,
            output_limit,
            initialized: false,
        }
    }

    pub fn reset(&mut self) {
        self.integral_error = 0.0;
        self.initialized = false;
    }

    pub fn compute(&mut self, setpoint: f32, measurement: f32, rate: f32, dt: f32) -> f32 {
        if !dt.is_finite() || dt <= 0.0 || dt > 0.05 {
            return 0.0;
        }

        let error = setpoint - measurement;
        let p = self.kp * error;

        self.integral_error += error * dt;
        self.integral_error = self
            .integral_error
            .clamp(-self.integral_limit, self.integral_limit);
        let i = self.ki * self.integral_error;

        let d = if self.initialized {
            -self.kd * rate
        } else {
            0.0
        };
        self.initialized = true;

        let unsaturated = p + i + d;
        let output = unsaturated.clamp(-self.output_limit, self.output_limit);

        // anti-windup: не копим интеграл в сторону насыщения
        if (unsaturated > self.output_limit && error > 0.0)
            || (unsaturated < -self.output_limit && error < 0.0)
        {
            self.integral_error -= error * dt;
        }

        output
    }
}
