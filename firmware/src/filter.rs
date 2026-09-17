//! Комплементарный фильтр угла с онлайн-оценкой дрейфа гироскопа.

const MAX_CORRECTION_DEG: f32 = 8.0;
const MAX_BIAS_DPS: f32 = 15.0;
const BIAS_GAIN: f32 = 0.2;

pub struct ComplementaryFilter {
    angle: f32,
    bias: f32,
    kp: f32,
}

impl ComplementaryFilter {
    pub fn new(tau: f32) -> Self {
        assert!(tau > 0.0 && tau.is_finite(), "tau должен быть > 0");
        Self {
            angle: 0.0,
            bias: 0.0,
            kp: 1.0 / tau,
        }
    }

    pub fn reset(&mut self, angle: f32) {
        self.angle = if angle.is_finite() { angle } else { 0.0 };
    }

    /// Сброс накопленного дрейфа нуля гироскопа (вызывается при падении/арминге)
    pub fn reset_bias(&mut self) {
        self.bias = 0.0;
    }

    pub fn set_tau(&mut self, tau: f32) {
        if tau.is_finite() && tau > 0.0 {
            self.kp = 1.0 / tau;
        }
    }

    pub fn bias(&self) -> f32 {
        self.bias
    }

    /// `bias_adapt` — можно ли сейчас обновлять оценку дрейфа гироскопа.
    /// Во время разгона/торможения акселерометр «видит» ускорение как наклон,
    /// и оценка дрейфа на таких данных уползает: после поездки угол начинает
    /// плыть. Поэтому дрейф дооцениваем только когда колёса почти стоят.
    pub fn update(
        &mut self,
        accel_angle: f32,
        gyro_rate: f32,
        accel_magnitude_g: f32,
        dt: f32,
        bias_adapt: bool,
    ) -> f32 {
        if !dt.is_finite() || dt <= 0.0 || dt > 0.05 {
            return self.angle;
        }
        if !accel_angle.is_finite() || !gyro_rate.is_finite() {
            return self.angle;
        }

        self.angle += (gyro_rate - self.bias) * dt;

        let deviation = (accel_magnitude_g - 1.0).abs();
        let weight = if deviation <= 0.08 {
            1.0
        } else if deviation < 0.40 {
            (0.40 - deviation) / 0.32
        } else {
            0.0
        };

        if weight > 0.0 {
            let error = (accel_angle - self.angle).clamp(-MAX_CORRECTION_DEG, MAX_CORRECTION_DEG);
            self.angle += self.kp * weight * error * dt;

            // ВАЖНО: Адаптируем дрейф нуля ТОЛЬКО около вертикали (< 15°)
            // и только в покое. Когда робот лежит на полу или едет, дрейф не копится!
            if bias_adapt && self.angle.abs() < 15.0 {
                self.bias = (self.bias - BIAS_GAIN * weight * error * dt)
                    .clamp(-MAX_BIAS_DPS, MAX_BIAS_DPS);
            }
        }

        self.angle
    }
}