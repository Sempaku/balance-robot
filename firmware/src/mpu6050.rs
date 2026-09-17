use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::i2c::I2cDriver;
use esp_idf_svc::sys::EspError;

const MPU6050_ADDR: u8 = 0x68;

const REG_SMPLRT_DIV: u8 = 0x19;
const REG_CONFIG: u8 = 0x1A;
const REG_GYRO_CONFIG: u8 = 0x1B;
const REG_ACCEL_CONFIG: u8 = 0x1C;
const REG_ACCEL_CONFIG_2: u8 = 0x1D;
const REG_ACCEL_XOUT_H: u8 = 0x3B;
const REG_PWR_MGMT_1: u8 = 0x6B;
const REG_WHO_AM_I: u8 = 0x75;

const I2C_TIMEOUT_MS: u32 = 10;
const CALIBRATION_SAMPLES: usize = 200;
/// Если разброс гироскопа при калибровке больше — робота трогали
pub const CALIBRATION_MAX_SPREAD_DPS: f32 = 3.0;
const CALIBRATION_ATTEMPTS: u32 = 3;

/// Наклон меряем по оси X акселерометра (вращение вокруг Y, гироскоп `gy`).
const TILT_USES_X: bool = true;
/// ±1000 °/с → 32.8 LSB/(°/с)
const GYRO_SCALE: f32 = 32.8;
/// ±2 g → 16384 LSB/g
const ACCEL_SCALE: f32 = 16384.0;

pub struct ImuData {
    pub accel_angle_deg: f32,
    pub gyro_rate_dps: f32,
    pub accel_magnitude_g: f32,
}

pub struct Mpu6050<'d> {
    i2c: I2cDriver<'d>,
    gyro_offset: f32,
    gyro_sign: f32,
}

impl<'d> Mpu6050<'d> {
    pub fn new(mut i2c: I2cDriver<'d>) -> Result<Self, EspError> {
        log::info!("Инициализация IMU...");

        i2c.write(MPU6050_ADDR, &[REG_PWR_MGMT_1, 0x01], I2C_TIMEOUT_MS)?;
        FreeRtos::delay_ms(50);

        let mut who_am_i = [0u8; 1];
        i2c.write_read(
            MPU6050_ADDR,
            &[REG_WHO_AM_I],
            &mut who_am_i,
            I2C_TIMEOUT_MS,
        )?;

        let id = who_am_i[0];
        log::info!("IMU WHO_AM_I = 0x{id:02X}");
        match id {
            0x68 => log::info!("Чип: MPU6050"),
            0x70 => log::info!("Чип: MPU6500 (клоны GY-521 часто врут маркировкой)"),
            0x71 => log::info!("Чип: MPU9250 / MPU6500-совместимый"),
            other => log::warn!("Неизвестный WHO_AM_I = 0x{other:02X}, продолжаем"),
        }

        // Гироскоп DLPF 20 Гц: на 41 Гц вибрация шаговиков пролезала в D-член
        // и раскачивала робота (G прыгал на ±100 °/с при почти неподвижном угле)
        i2c.write(MPU6050_ADDR, &[REG_CONFIG, 0x04], I2C_TIMEOUT_MS)?;
        // 1 кГц / (1 + 4) = 200 Гц — под контур управления
        i2c.write(MPU6050_ADDR, &[REG_SMPLRT_DIV, 4], I2C_TIMEOUT_MS)?;
        i2c.write(MPU6050_ADDR, &[REG_GYRO_CONFIG, 0x10], I2C_TIMEOUT_MS)?;
        i2c.write(MPU6050_ADDR, &[REG_ACCEL_CONFIG, 0x00], I2C_TIMEOUT_MS)?;

        if id == 0x70 || id == 0x71 || id == 0x73 {
            // Акселерометр DLPF 20 Гц: шаговики дают вибрацию до 2.5 g,
            // при 41 Гц она пролезала в угол и ломала фильтр.
            i2c.write(MPU6050_ADDR, &[REG_ACCEL_CONFIG_2, 0x04], I2C_TIMEOUT_MS)?;
            log::info!("MPU6500: accel/gyro DLPF 20 Гц, ±1000 °/с");
        }

        FreeRtos::delay_ms(50);

        let mut sensor = Self {
            i2c,
            gyro_offset: 0.0,
            gyro_sign: 1.0,
        };
        sensor.calibrate()?;
        Ok(sensor)
    }

    pub fn gyro_sign(&self) -> f32 {
        self.gyro_sign
    }

    pub fn flip_gyro_sign(&mut self) {
        self.gyro_sign = -self.gyro_sign;
    }

    pub fn set_offset(&mut self, offset: f32) {
        self.gyro_offset = offset;
    }

    pub fn offset(&self) -> f32 {
        self.gyro_offset
    }

    fn calibrate(&mut self) -> Result<(), EspError> {
        self.calibrate_with_result()
            .map(|_| ())
    }

    /// Калибровка гироскопа с возвратом результата (разброс).
    /// Возвращает Ok(spread) при успехе или Err при ошибке.
    pub fn calibrate_with_result(&mut self) -> Result<f32, EspError> {
        for attempt in 1..=CALIBRATION_ATTEMPTS {
            log::info!("Калибровка гироскопа ({attempt}) — НЕ ДВИГАЙ РОБОТА ~1 с");

            let mut sum = 0.0f32;
            let mut min = f32::MAX;
            let mut max = f32::MIN;

            for _ in 0..CALIBRATION_SAMPLES {
                let (_ax, _ay, _az, gx, gy, _gz) = self.read_raw()?;
                let v = if TILT_USES_X { gy } else { gx };
                sum += v;
                min = min.min(v);
                max = max.max(v);
                FreeRtos::delay_ms(5);
            }

            self.gyro_offset = sum / CALIBRATION_SAMPLES as f32;
            let spread = max - min;
            log::info!(
                "Gyro offset = {:+.3} °/с, разброс {:.2} °/с",
                self.gyro_offset,
                spread
            );

            if spread <= CALIBRATION_MAX_SPREAD_DPS {
                return Ok(spread);
            }
            log::warn!("Робота двигали при калибровке — повтор");
        }

        log::warn!("Калибровка шумная, берём последнее значение offset");
        Ok(f32::MAX) // Возвращаем максимальный разброс при неудаче
    }

    pub fn read_raw(&mut self) -> Result<(f32, f32, f32, f32, f32, f32), EspError> {
        let mut buf = [0u8; 14];
        self.i2c.write_read(
            MPU6050_ADDR,
            &[REG_ACCEL_XOUT_H],
            &mut buf,
            I2C_TIMEOUT_MS,
        )?;

        let accel_x = i16::from_be_bytes([buf[0], buf[1]]) as f32 / ACCEL_SCALE;
        let accel_y = i16::from_be_bytes([buf[2], buf[3]]) as f32 / ACCEL_SCALE;
        let accel_z = i16::from_be_bytes([buf[4], buf[5]]) as f32 / ACCEL_SCALE;
        let gyro_x = i16::from_be_bytes([buf[8], buf[9]]) as f32 / GYRO_SCALE;
        let gyro_y = i16::from_be_bytes([buf[10], buf[11]]) as f32 / GYRO_SCALE;
        let gyro_z = i16::from_be_bytes([buf[12], buf[13]]) as f32 / GYRO_SCALE;

        Ok((accel_x, accel_y, accel_z, gyro_x, gyro_y, gyro_z))
    }

    pub fn read(&mut self) -> Result<ImuData, EspError> {
        let (ax, ay, az, gx, gy, _gz) = self.read_raw()?;

        // Знаки: при повороте вокруг +Y угол atan2(ax, az) УБЫВАЕТ, а gy растёт.
        // Поэтому для оси X берём atan2(-ax, az) — иначе гироскоп и
        // акселерометр тянут фильтр в разные стороны.
        let (accel_angle, gyro_raw) = if TILT_USES_X {
            ((-ax).atan2(az) * (180.0 / std::f32::consts::PI), gy)
        } else {
            (ay.atan2(az) * (180.0 / std::f32::consts::PI), gx)
        };

        let accel_magnitude = (ax * ax + ay * ay + az * az).sqrt();
        let gyro_rate = self.gyro_sign * (gyro_raw - self.gyro_offset);

        Ok(ImuData {
            accel_angle_deg: accel_angle,
            gyro_rate_dps: gyro_rate,
            accel_magnitude_g: accel_magnitude,
        })
    }

    pub fn read_angle_and_rate(&mut self) -> Result<(f32, f32), EspError> {
        let data = self.read()?;
        Ok((data.accel_angle_deg, data.gyro_rate_dps))
    }
}
