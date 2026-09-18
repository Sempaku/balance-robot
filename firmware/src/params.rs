//! Единый реестр настраиваемых величин.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::control::{DriveParams, Gains};

pub struct ParamInfo {
    pub key: &'static str,
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub about: &'static str,
}

macro_rules! declare_params {
    ($(
        $field:ident, $key:literal, $unit:literal, $min:expr, $max:expr, $default:expr, $about:literal;
    )*) => {
        #[derive(Clone, Copy, Debug)]
        pub struct Params {
            $(pub $field: f32,)*
        }

        impl Default for Params {
            fn default() -> Self {
                Self { $($field: $default,)* }
            }
        }

        pub const CATALOG: &[ParamInfo] = &[
            $(ParamInfo {
                key: $key,
                unit: $unit,
                min: $min,
                max: $max,
                default: $default,
                about: $about,
            },)*
        ];

        pub struct ParamStore {
            $($field: AtomicU32,)*
        }

        impl ParamStore {
            pub fn new(p: Params) -> Arc<Self> {
                Arc::new(Self {
                    $($field: AtomicU32::new(p.$field.to_bits()),)*
                })
            }

            pub fn snapshot(&self) -> Params {
                Params {
                    $($field: f32::from_bits(self.$field.load(Ordering::Relaxed)),)*
                }
            }

            pub fn apply(&self, p: Params) {
                $(self.$field.store(p.$field.to_bits(), Ordering::Relaxed);)*
            }

            pub fn get(&self, key: &str) -> Option<f32> {
                match key {
                    $($key => Some(f32::from_bits(self.$field.load(Ordering::Relaxed))),)*
                    _ => None,
                }
            }

            pub fn set(&self, key: &str, value: f32) -> Option<f32> {
                if !value.is_finite() {
                    return None;
                }
                match key {
                    $($key => {
                        let v = value.clamp($min, $max);
                        self.$field.store(v.to_bits(), Ordering::Relaxed);
                        Some(v)
                    })*
                    _ => None,
                }
            }
        }
    };
}

declare_params! {
    ka_angle,       "ka",           "шаг/с²/с²/°",  100.0,   12000.0, 3800.0,
        "Жёсткость по углу (3200 шаг/об)";
    kd_gyro,        "kd",           "шаг/с²/°/с", 0.0,    3000.0,  600.0,
        "Демпфирование по гироскопу";
    ki_angle,       "ki",           "шаг/с²/°·с",  0.0,    50.0,    0.0,
        "Интегральная составляющая по углу (устраняет статическую ошибку). Начните с 0.0, увеличивайте с шагом 1.0";
    kp_pos,         "kp",           "°/шаг",      0.0,    0.01,    0.0005,
        "Внешний контур: наклон в ответ на смещение точки стоянки";
    kd_vel,         "kv",           "°/(шаг/с)",  0.0,    0.01,    0.0010,
        "Наклон корпуса на недобор скорости: градусов на каждый шаг/с отставания колёс от цели хода";
    trim_offset,    "trim_offset",  "°",          -8.0,   8.0,     0.0,
        "Смещение точки равновесия (трим). Положительное — наклон вперед, отрицательное — назад";
    max_lean_deg,   "lean",         "°",          0.5,    18.0,    9.0,
        "Предельный наклон корпуса робота при штурме неровностей";
    max_drive_lean, "drive_lean",   "°",          0.5,    8.0,     3.0,
        "Базовый наклон хода при свободной езде";
    max_turn_rate,  "turn_rate",    "шаг/с",      100.0,  8000.0,  3000.0,
        "Скорость разворота при полном отклонении стика вбок";
    drive_expo,     "expo",         "",           0.0,    0.8,     0.0,
        "Экспонента газа: 0=линейно, 0.8=макс.чувствительность на малых ходах";
    trim_adapt_drive, "trim_adapt", "°/с/°",      0.0,    1.0,     0.2,
        "Адаптация трима на ходу: постоянный наклон у целевой скорости переучивается в трим (симметрирует тягу). 0 = выкл";
    fall_angle_deg, "fall",         "°",          5.0,    45.0,    22.0,
        "Порог падения: отклонение от целевого угла хода";
    fall_gyro_dps,  "fall_gyro",    "°/с",        50.0,   800.0,   260.0,
        "Аварийный стоп при резком неконтролируемом срыве гироскопа";
    arm_max_angle,  "arm_angle",    "°",          2.0,    30.0,    14.0,
        "Максимальный наклон при арминге";
    filter_tau,     "tau",          "с",          0.1,    10.0,    1.5,
        "Постоянная времени фильтра угла";
    max_wheel_rps,  "max_rps",      "об/с",       0.5,    6.0,     3.0,
        "Потолок скорости колес (выше мотор срывается)";
    telemetry_hz,   "telemetry_hz", "Гц",         1.0,    200.0,   100.0,
        "Частота отправки телеметрии по UDP";
}

impl Params {
    pub fn gains(&self) -> Gains {
        Gains {
            ka_angle: self.ka_angle,
            kd_gyro: self.kd_gyro,
            ki_angle: self.ki_angle,
            kp_pos: self.kp_pos,
            kd_vel: self.kd_vel,
            max_lean_deg: self.max_lean_deg,
        }
    }

    pub fn drive_params(&self) -> DriveParams {
        DriveParams {
            max_drive_lean_deg: self.max_drive_lean,
            max_turn_rate: self.max_turn_rate,
            drive_expo: self.drive_expo,
            trim_adapt_drive: self.trim_adapt_drive,
        }
    }
}

impl ParamStore {
    pub fn reset(&self) {
        self.apply(Params::default());
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for info in CATALOG {
            if let Some(v) = self.get(info.key) {
                map.insert(info.key.to_string(), json_number(v));
            }
        }
        serde_json::Value::Object(map)
    }

    pub fn catalog_json() -> serde_json::Value {
        let items: Vec<serde_json::Value> = CATALOG
            .iter()
            .map(|i| {
                serde_json::json!({
                    "key": i.key,
                    "unit": i.unit,
                    "min": i.min,
                    "max": i.max,
                    "default": i.default,
                    "about": i.about,
                })
            })
            .collect();
        serde_json::Value::Array(items)
    }
}

fn json_number(v: f32) -> serde_json::Value {
    serde_json::Number::from_f64(v as f64)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}