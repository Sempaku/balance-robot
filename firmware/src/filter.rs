//! Комплементарный фильтр угла с онлайн-оценкой дрейфа гироскопа
//! и адаптацией к вибрациям шаговых двигателей.

const MAX_CORRECTION_DEG: f32 = 8.0;
/// Лимит коррекции вне баланса: после падения/подъёма угол может расходиться
/// с акселерометром на 30–90°, и обычный лимит 8° стягивает его со скоростью
/// ~5°/с — приходится ждать, пока фильтр «доедет» до вертикали перед армингом.
/// В покое разрешаем тянуть сильно быстрее (тау остаётся той же).
const MAX_CORRECTION_IDLE_DEG: f32 = 90.0;
const MAX_BIAS_DPS: f32 = 15.0;
const BIAS_GAIN: f32 = 0.2;

/// Базовый порог вибрации (в покое)
const VIBRATION_BASE_THRESHOLD: f32 = 0.08;
/// Верхний порог вибрации для полного отключения коррекции
const VIBRATION_MAX_THRESHOLD: f32 = 0.40;
/// Скорость адаптации порога вибрации (1/с)
const VIBRATION_ADAPT_RATE: f32 = 2.0;
/// Минимальный вес коррекции (никогда не отключаем полностью)
const MIN_CORRECTION_WEIGHT: f32 = 0.1;
/// Постоянная времени сглаживания веса (с)
const WEIGHT_FILTER_TAU: f32 = 0.2;

pub struct ComplementaryFilter {
    angle: f32,
    bias: f32,
    kp: f32,
    vibration_level: f32,
    correction_weight: f32,
}

impl ComplementaryFilter {
    pub fn new(tau: f32) -> Self {
        assert!(tau > 0.0 && tau.is_finite(), "tau должен быть > 0");
        Self {
            angle: 0.0,
            bias: 0.0,
            kp: 1.0 / tau,
            vibration_level: 0.0,
            correction_weight: 1.0,
        }
    }

    pub fn reset(&mut self, angle: f32) {
        self.angle = if angle.is_finite() { angle } else { 0.0 };
        self.vibration_level = 0.0;
        self.correction_weight = 1.0;
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

    /// Текущий уровень вибрации (0.0 = нет вибрации, >0.4 = сильная)
    #[allow(dead_code)]
    pub fn vibration_level(&self) -> f32 {
        self.vibration_level
    }

    /// Текущий вес коррекции акселерометром (0.0..1.0)
    #[allow(dead_code)]
    pub fn correction_weight(&self) -> f32 {
        self.correction_weight
    }

    /// `bias_adapt` — можно ли сейчас обновлять оценку дрейфа гироскопа.
    /// Во время разгона/торможения акселерометр «видит» ускорение как наклон,
    /// и оценка дрейфа на таких данных уползает: после поездки угол начинает
    /// плыть. Поэтому дрейф дооцениваем только когда колёса почти стоят.
    ///
    /// `fast_converge` — быстрое схождение к акселерометру (вне баланса):
    /// после падения и подъёма робота не ждём, пока лимит 8° вытянет угол
    /// к вертикали. В балансе не включать: лимит защищает от выбросов
    /// акселерометра на вибрациях.
    ///
    /// Адаптация к вибрациям:
    /// - Оцениваем текущий уровень вибрации по отклонению accel_magnitude от 1g
    /// - Сглаживаем оценку экспоненциальным фильтром
    /// - Уменьшаем вес коррекции при высокой вибрации
    /// - Динамически подстраиваем порог срабатывания фильтра
    pub fn update(
        &mut self,
        accel_angle: f32,
        gyro_rate: f32,
        accel_magnitude_g: f32,
        dt: f32,
        bias_adapt: bool,
        fast_converge: bool,
    ) -> f32 {
        if !dt.is_finite() || dt <= 0.0 || dt > 0.05 {
            return self.angle;
        }
        if !accel_angle.is_finite() || !gyro_rate.is_finite() {
            return self.angle;
        }

        self.angle += (gyro_rate - self.bias) * dt;

        // Оценка текущего уровня вибрации
        let vibration = (accel_magnitude_g - 1.0).abs();
        
        // Экспоненциальное сглаживание уровня вибрации
        let alpha = (VIBRATION_ADAPT_RATE * dt).min(0.5);
        self.vibration_level += (vibration - self.vibration_level) * alpha;

        // Адаптивный порог: базовый + доля от текущего уровня вибрации
        let adaptive_threshold = VIBRATION_BASE_THRESHOLD + 0.3 * self.vibration_level;

        // Вычисление веса коррекции (плавная функция)
        let raw_weight = if self.vibration_level <= adaptive_threshold {
            1.0
        } else if self.vibration_level < VIBRATION_MAX_THRESHOLD {
            ((VIBRATION_MAX_THRESHOLD - self.vibration_level) 
                / (VIBRATION_MAX_THRESHOLD - adaptive_threshold)).max(MIN_CORRECTION_WEIGHT)
        } else {
            MIN_CORRECTION_WEIGHT
        };

        // Low-pass фильтр веса для плавности
        let target_weight = raw_weight;
        let filter_alpha = (dt / WEIGHT_FILTER_TAU).min(0.5);
        self.correction_weight += (target_weight - self.correction_weight) * filter_alpha;

        if self.correction_weight > 0.0 {
            let error_raw = accel_angle - self.angle;

            // В покое тянем к акселерометру с широким лимитом (быстрое
            // схождение после падения), в балансе — с прежним 8°
            let max_corr = if fast_converge {
                MAX_CORRECTION_IDLE_DEG
            } else {
                MAX_CORRECTION_DEG
            };
            let error = error_raw.clamp(-max_corr, max_corr);
            self.angle += self.kp * self.correction_weight * error * dt;

            // ВАЖНО: Адаптируем дрейф нуля ТОЛЬКО около вертикали (< 15°),
            // только в поке и только когда угол уже сошёлся к акселерометру
            // (иначе быстрое схождение с большого расхождения отравило бы
            // оценку дрейфа). Когда робот лежит на полу или едет, дрейф не копится!
            if bias_adapt
                && self.angle.abs() < 15.0
                && self.correction_weight > 0.5
                && error_raw.abs() <= MAX_CORRECTION_DEG
            {
                self.bias = (self.bias - BIAS_GAIN * self.correction_weight * error * dt)
                    .clamp(-MAX_BIAS_DPS, MAX_BIAS_DPS);
            }
        }

        self.angle
    }
}