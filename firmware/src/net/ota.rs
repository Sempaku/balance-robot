//! Приём новой прошивки по сети.
//!
//! Ради этого модуля всё и затевалось: раньше каждая правка означала поход к
//! плате с кнопками BOOT/EN. Теперь прошивка приезжает в свободный слот, и
//! бутлоадер откатится на старую, если новая не подтвердит работоспособность.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::ota::EspOta;

/// Размер куска, которым читаем из сокета. Влезает в стек задачи и совпадает
/// с типичным окном TCP, так что запись во флеш идёт без простоев.
const CHUNK: usize = 2048;
/// Заведомо больше любой нашей прошивки, но меньше слота: защита от мусора
/// в поле `size`, из-за которого мы бы стирали флеш впустую.
const MAX_IMAGE: u64 = 1_900_000;

/// Идёт ли обновление прямо сейчас. Нужен, чтобы не начать второе и чтобы
/// показать состояние в телеметрии.
static IN_PROGRESS: AtomicBool = AtomicBool::new(false);

pub fn in_progress() -> bool {
    IN_PROGRESS.load(Ordering::Acquire)
}

/// Отмечает текущую прошивку рабочей, отменяя откат.
///
/// Вызывается после того, как робот доказал жизнеспособность: поднял IMU,
/// прошёл контур управления, подключился к сети. Если прошивка падает раньше,
/// бутлоадер на следующем старте вернёт предыдущий слот.
pub fn confirm_running_image() {
    match EspOta::new().and_then(|mut ota| ota.mark_running_slot_valid()) {
        Ok(()) => log::info!("OTA: текущая прошивка подтверждена"),
        // Не ошибка: при сборке без отката или при первой прошивке кнопками
        // подтверждать нечего.
        Err(e) => log::debug!("OTA: подтверждение не потребовалось ({e})"),
    }
}

pub fn running_slot_label() -> String {
    EspOta::new()
        .and_then(|ota| ota.get_running_slot())
        .map(|slot| slot.label.to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

pub fn update_slot_label() -> String {
    EspOta::new()
        .and_then(|ota| ota.get_update_slot())
        .map(|slot| slot.label.to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Читает из потока ровно `size` байт образа и записывает их в свободный слот.
///
/// Возвращает управление только после полной записи: при обрыве связи слот
/// остаётся неактивированным, а робот продолжает работать на текущей прошивке.
pub fn receive<R: Read>(source: &mut R, size: u64) -> anyhow::Result<()> {
    if size == 0 || size > MAX_IMAGE {
        anyhow::bail!("неправдоподобный размер образа: {size} байт");
    }
    if IN_PROGRESS.swap(true, Ordering::AcqRel) {
        anyhow::bail!("обновление уже идёт");
    }

    let result = write_image(source, size);
    IN_PROGRESS.store(false, Ordering::Release);
    result
}

fn write_image<R: Read>(source: &mut R, size: u64) -> anyhow::Result<()> {
    let mut ota = EspOta::new()?;
    let target = ota.get_update_slot()?.label.to_string();
    log::warn!("OTA: приём {size} байт в слот {target}");

    let mut update = ota.initiate_update()?;
    let mut buf = [0u8; CHUNK];
    let mut written = 0u64;
    let mut last_report = 0u64;

    while written < size {
        let want = ((size - written) as usize).min(CHUNK);
        let read = match source.read(&mut buf[..want]) {
            Ok(0) => {
                update.abort()?;
                anyhow::bail!("связь оборвалась на {written} из {size} байт");
            }
            Ok(n) => n,
            Err(e) => {
                update.abort()?;
                return Err(e.into());
            }
        };

        if let Err(e) = update.write(&buf[..read]) {
            update.abort()?;
            return Err(e.into());
        }
        written += read as u64;

        if written - last_report >= size / 10 + 1 {
            last_report = written;
            log::info!("OTA: {}%", written * 100 / size);
        }
    }

    update.complete()?;
    log::warn!("OTA: образ записан, перезагрузка");
    Ok(())
}

/// Перезагружается с задержкой, чтобы ответ успел уйти в сокет.
pub fn reboot_soon(delay: Duration) -> ! {
    FreeRtos::delay_ms(delay.as_millis() as u32);
    esp_idf_svc::hal::reset::restart()
}
