//! Энергонезависимое хранилище: коэффициенты регулятора и список Wi-Fi сетей.

use std::sync::Mutex;

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use serde::{Deserialize, Serialize};

use crate::params::ParamStore;

const NAMESPACE: &str = "robot";
const KEY_PARAMS: &str = "params";
const KEY_WIFI: &str = "wifi";
const READ_BUF: usize = 1536;

/// Одна сохранённая сеть. `priority` больше — пробуем раньше.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WifiNetwork {
    pub ssid: String,
    #[serde(default)]
    pub psk: String,
    #[serde(default)]
    pub priority: i32,
}

pub struct Storage {
    nvs: Mutex<EspNvs<NvsDefault>>,
}

impl Storage {
    pub fn new(partition: EspDefaultNvsPartition) -> anyhow::Result<Self> {
        let nvs = EspNvs::new(partition, NAMESPACE, true)?;
        Ok(Self {
            nvs: Mutex::new(nvs),
        })
    }

    fn read_json(&self, key: &str) -> Option<serde_json::Value> {
        let nvs = self.nvs.lock().ok()?;
        let mut buf = [0u8; READ_BUF];
        let raw = nvs.get_str(key, &mut buf).ok()??;
        serde_json::from_str(raw).ok()
    }

    fn write_json(&self, key: &str, value: &serde_json::Value) -> anyhow::Result<()> {
        let text = serde_json::to_string(value)?;
        if text.len() >= READ_BUF {
            anyhow::bail!("{key}: {} байт не влезет в буфер чтения", text.len());
        }
        let nvs = self
            .nvs
            .lock()
            .map_err(|_| anyhow::anyhow!("NVS mutex отравлен"))?;
        nvs.set_str(key, &text)?;
        Ok(())
    }

    pub fn load_params(&self, store: &ParamStore) -> usize {
        let Some(serde_json::Value::Object(map)) = self.read_json(KEY_PARAMS) else {
            return 0;
        };

        let mut applied = 0;
        for (key, value) in map {
            let Some(v) = value.as_f64() else { continue };
            if store.set(&key, v as f32).is_some() {
                applied += 1;
            } else {
                log::warn!("NVS: параметр `{key}` больше не поддерживается, пропущен");
            }
        }
        applied
    }

    pub fn save_params(&self, store: &ParamStore) -> anyhow::Result<()> {
        self.write_json(KEY_PARAMS, &store.to_json())
    }

    pub fn clear_params(&self) -> anyhow::Result<()> {
        let nvs = self
            .nvs
            .lock()
            .map_err(|_| anyhow::anyhow!("NVS mutex отравлен"))?;
        nvs.remove(KEY_PARAMS)?;
        Ok(())
    }

    pub fn load_networks(&self) -> Vec<WifiNetwork> {
        let Some(value) = self.read_json(KEY_WIFI) else {
            return Vec::new();
        };
        serde_json::from_value(value).unwrap_or_else(|e| {
            log::warn!("NVS: список сетей повреждён ({e}), игнорирую");
            Vec::new()
        })
    }

    /// Возвращает все известные роботу сети (NVS + вшитые при сборке).
    pub fn all_known_networks(&self) -> Vec<WifiNetwork> {
        let mut networks = self.load_networks();
        for seed in seed_networks() {
            if !networks.iter().any(|n| n.ssid == seed.ssid) {
                networks.push(seed);
            }
        }
        networks
    }

    pub fn save_networks(&self, networks: &[WifiNetwork]) -> anyhow::Result<()> {
        let value = serde_json::to_value(networks)?;
        self.write_json(KEY_WIFI, &value)
    }
}

pub fn seed_networks() -> Vec<WifiNetwork> {
    let Some(raw) = option_env!("WIFI_SEED_JSON") else {
        return Vec::new();
    };
    serde_json::from_str(raw).unwrap_or_else(|e| {
        log::warn!("wifi_seed.json не разобран ({e})");
        Vec::new()
    })
}