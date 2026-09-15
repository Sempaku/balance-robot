fn main() {
    embuild::espidf::sysenv::output();
    embed_wifi_seed();
}

/// Вшивает `wifi_seed.json` в прошивку, чтобы чистая плата сразу нашла
/// домашнюю сеть. Файл не обязателен: без него робот поднимет портал.
fn embed_wifi_seed() {
    println!("cargo:rerun-if-changed=wifi_seed.json");

    let Ok(raw) = std::fs::read_to_string("wifi_seed.json") else {
        return;
    };

    // Значение уходит в переменную окружения компилятора, поэтому переносы
    // строк нужно убрать — заодно это проверяет, что JSON вообще валиден.
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            println!("cargo:warning=wifi_seed.json не разобран: {e}");
            return;
        }
    };

    println!("cargo:rustc-env=WIFI_SEED_JSON={value}");
}
