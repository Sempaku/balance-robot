//! Комплементарный фильтр угла с онлайн-оценкой дрейфа гироскопа.
//!
//! Вибрация шаговиков даёт выбросы угла по акселерометру на десятки градусов.
//! Раньше такие выборки просто отбрасывались — в итоге угол считался почти
//! чистым интегралом гироскопа и медленно уезжал.
//!
//! Здесь коррекция не отбрасывается, а ограничивается по величине: выброс
//! в 60° двигает оценку не быстрее, чем `kp * MAX_CORRECTION_DEG` °/с.
//! Интегральная часть оценивает остаточный дрейф гироскопа — без неё
//! ошибка калибровки в 1 °/с за 30 секунд превращается в 30° ухода.

/// Насколько сильно один выброс акселерометра может тянуть оценку
const MAX_CORRECTION_DEG: f32 = 8.0;
/// Предел оценки дрейфа
const MAX_BIAS_DPS: f32 = 15.0;
/// Скорость сходимости оценки дрейфа, 1/с²
const BIAS_GAIN: f32 = 0.2;

pub struct ComplementaryFilter {
    angle: f32,
    bias: f32,
    kp: f32,
}

impl ComplementaryFilter {
    /// `tau` — постоянная времени подтяжки к акселерометру (с)
    pub fn new(tau: f32) -> Self {
        assert!(tau > 0.0 && tau.is_finite(), "tau должен быть > 0");
        Self {
            angle: 0.0,
            bias: 0.0,
            kp: 1.0 / tau,
        }
    }

    /// Сбрасывает угол, но сохраняет накопленную оценку дрейфа.
    pub fn reset(&mut self, angle: f32) {
        self.angle = if angle.is_finite() { angle } else { 0.0 };
    }

    /// Меняет доверие к акселерометру на ходу (подстройка из UI).
    pub fn set_tau(&mut self, tau: f32) {
        if tau.is_finite() && tau > 0.0 {
            self.kp = 1.0 / tau;
        }
    }

    pub fn bias(&self) -> f32 {
        self.bias
    }

    pub fn update(
        &mut self,
        accel_angle: f32,
        gyro_rate: f32,
        accel_magnitude_g: f32,
        dt: f32,
    ) -> f32 {
        if !dt.is_finite() || dt <= 0.0 || dt > 0.05 {
            return self.angle;
        }
        if !accel_angle.is_finite() || !gyro_rate.is_finite() {
            return self.angle;
        }

        self.angle += (gyro_rate - self.bias) * dt;

        // Чем дальше модуль ускорения от 1g, тем меньше веры акселерометру
        let deviation = (accel_magnitude_g - 1.0).abs();
        let weight = if deviation <= 0.05 {
            1.0
        } else if deviation < 0.40 {
            (0.40 - deviation) / 0.35
        } else {
            0.0
        };

        if weight > 0.0 {
            let error =
                (accel_angle - self.angle).clamp(-MAX_CORRECTION_DEG, MAX_CORRECTION_DEG);
            self.angle += self.kp * weight * error * dt;
            self.bias = (self.bias - BIAS_GAIN * weight * error * dt)
                .clamp(-MAX_BIAS_DPS, MAX_BIAS_DPS);
        }

        self.angle
    }
}
