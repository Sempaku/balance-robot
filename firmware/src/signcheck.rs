//! Проверка согласованности знаков акселерометра и гироскопа.
//!
//! Комплементарный фильтр интегрирует гироскоп и подтягивает результат к углу
//! по акселерометру. Если у них разные знаки, фильтр воюет сам с собой и угол
//! получается мусорным — робот «дёргается» и падает.
//!
//! Проверка работает только на движении: нужно покачать робота руками.

pub struct GyroSignCheck {
    prev_angle: Option<f32>,
    corr: f32,
    samples: u32,
    decided: bool,
}

impl GyroSignCheck {
    /// Сколько «быстрых» выборок нужно набрать для вердикта.
    pub const NEEDED: u32 = 80;

    pub fn new() -> Self {
        Self {
            prev_angle: None,
            corr: 0.0,
            samples: 0,
            decided: false,
        }
    }

    /// Возвращает `Some(sign)`, когда данных хватило.
    /// `sign == -1.0` означает, что знак гироскопа надо инвертировать.
    pub fn update(
        &mut self,
        accel_angle: f32,
        gyro_dps: f32,
        accel_magnitude_g: f32,
        dt: f32,
    ) -> Option<f32> {
        if self.decided || dt <= 0.0 || !dt.is_finite() {
            return None;
        }

        let prev = self.prev_angle.replace(accel_angle);

        // При сильной тряске угол по акселерометру бессмысленный
        if !(0.6..=1.6).contains(&accel_magnitude_g) {
            return None;
        }

        let prev = prev?;
        let delta = accel_angle - prev;
        // перескок atan2 через ±180°
        if delta.abs() > 45.0 {
            return None;
        }

        let accel_rate = delta / dt;
        // Считаем только заметное вращение, иначе корреляция — это шум
        if gyro_dps.abs() < 25.0 || accel_rate.abs() < 25.0 {
            return None;
        }

        self.corr += gyro_dps * accel_rate;
        self.samples += 1;

        if self.samples >= Self::NEEDED {
            self.decided = true;
            return Some(if self.corr < 0.0 { -1.0 } else { 1.0 });
        }
        None
    }
}
