use embedded_hal::i2c::I2c;

// ===== AS7343 register map (bank0/bank1) =====
const AS7343_I2C_ADDR: u8 = 0x39;
const AS7343_CHIP_ID: u8 = 0x81;

const REG_ENABLE: u8 = 0x80; // bank0
const REG_STATUS2: u8 = 0x90; // bank0, bit6 = AVALID
const REG_DATA_0_L: u8 = 0x95; // bank0, spectral data start
const REG_CFG0: u8 = 0xBF; // bank0, bit4 = REG_BANK
const REG_CFG1: u8 = 0xC6; // bank0, AGAIN[4:0]
const REG_CFG20: u8 = 0xD6; // bank0, auto_smux[6:5]
const REG_ASTEP_L: u8 = 0xD4; // bank0
const REG_ASTEP_H: u8 = 0xD5; // bank0
const REG_ATIME: u8 = 0x81; // bank0
const REG_LED: u8 = 0xCD;   // bank0, LED control
const REG_ID: u8 = 0x5A; // bank1

// ===== default measurement settings =====
const DEFAULT_ATIME: u8 = 29;       // Tint = (ATIME + 1) * (ASTEP + 1) * 2.78uSec
const DEFAULT_ASTEP: u16 = 599;
const DEFAULT_AGAIN: u8 = 9; // 256x
const DEFAULT_LED_CURRENT: u8 = 0x20; // LED電流設定(0..0x7F)

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bank {
    Bank0 = 0,
    Bank1 = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum As7343Error<E> {
    I2cWriteRead(E),
    I2cWrite(E),
    I2cRead(E),
    InvalidChipId(u8),
    DataNotReady,
}

fn as7343_read_u8<I: I2c>(i2c: &mut I, reg: u8) -> Result<u8, As7343Error<I::Error>> {
    let mut v = [0u8; 1];
    i2c.write_read(AS7343_I2C_ADDR, &[reg], &mut v)
        .map_err(As7343Error::I2cWriteRead)?;
    Ok(v[0])
}

fn as7343_write_u8<I: I2c>(i2c: &mut I, reg: u8, val: u8) -> Result<(), As7343Error<I::Error>> {
    i2c.write(AS7343_I2C_ADDR, &[reg, val])
        .map_err(As7343Error::I2cWrite)
}

fn as7343_read_block<I: I2c>(
    i2c: &mut I,
    reg: u8,
    buf: &mut [u8],
) -> Result<(), As7343Error<I::Error>> {
    i2c.write_read(AS7343_I2C_ADDR, &[reg], buf)
        .map_err(As7343Error::I2cRead)
}

pub fn as7343_set_bank<I: I2c>(i2c: &mut I, bank: Bank) -> Result<(), As7343Error<I::Error>> {
    let mut cfg0 = as7343_read_u8(i2c, REG_CFG0)?;
    if bank == Bank::Bank1 {
        cfg0 |= 1 << 4; // REG_BANK=1
    } else {
        cfg0 &= !(1 << 4); // REG_BANK=0
    }
    as7343_write_u8(i2c, REG_CFG0, cfg0)
}

/// 最低限の初期化（ID確認 + 電源ON + 積分設定 + 18ch auto-smux + 測定開始）
pub fn as7343_init_default<I: I2c>(i2c: &mut I) -> Result<(), As7343Error<I::Error>> {
    // ID確認（bank1）
    as7343_set_bank(i2c, Bank::Bank1).unwrap();
    let id = as7343_read_u8(i2c, REG_ID)?;
    if id != AS7343_CHIP_ID {
        return Err(As7343Error::InvalidChipId(id));
    }

    // bank0へ戻す
    as7343_set_bank(i2c, Bank::Bank0)?;

    // PON=1 (ENABLE bit0)
    let mut en = as7343_read_u8(i2c, REG_ENABLE)?;
    en |= 1 << 0;
    as7343_write_u8(i2c, REG_ENABLE, en)?;

    // 積分時間の例（約50ms相当）
    as7343_write_u8(i2c, REG_ATIME, DEFAULT_ATIME)?;
    as7343_write_u8(i2c, REG_ASTEP_L, (DEFAULT_ASTEP & 0xFF) as u8)?;
    as7343_write_u8(i2c, REG_ASTEP_H, (DEFAULT_ASTEP >> 8) as u8)?;

    // ゲイン 256x (AGAIN=9)
    let mut cfg1 = as7343_read_u8(i2c, REG_CFG1)?;
    cfg1 = (cfg1 & !0x1F) | DEFAULT_AGAIN;
    as7343_write_u8(i2c, REG_CFG1, cfg1)?;

    // auto_smux = 18ch (bits[6:5] = 0b11)
    let mut cfg20 = as7343_read_u8(i2c, REG_CFG20)?;
    cfg20 = (cfg20 & !(0b11 << 5)) | (0b11 << 5);
    as7343_write_u8(i2c, REG_CFG20, cfg20)?;

    // SP_EN=1 (ENABLE bit1)
    en |= 1 << 1;
    as7343_write_u8(i2c, REG_ENABLE, en)?;

    // LED control (OFF, 電流値のみ設定)
    as7343_write_u8(i2c, REG_LED, DEFAULT_LED_CURRENT & 0x7F)?;

    Ok(())
}

/// AS7343内蔵LEDのON/OFF
pub fn as7343_set_led_enable<I: I2c>(
    i2c: &mut I,
    enable: bool,
) -> Result<(), As7343Error<I::Error>> {
    let mut led = as7343_read_u8(i2c, REG_LED)?;

    // 下位7bitは電流設定。0の場合は見えにくいためデフォルトを入れる
    if (led & 0x7F) == 0 {
        led = (led & 0x80) | (DEFAULT_LED_CURRENT & 0x7F);
    }

    if enable {
        led |= 1 << 7;
    } else {
        led &= !(1 << 7);
    }

    as7343_write_u8(i2c, REG_LED, led)
}

/// データ準備完了フラグ（STATUS2 bit6 = AVALID）
pub fn as7343_data_ready<I: I2c>(i2c: &mut I) -> Result<bool, As7343Error<I::Error>> {
    let st2 = as7343_read_u8(i2c, REG_STATUS2)?;
    Ok((st2 & (1 << 6)) != 0)
}

/// 18chデータを1回読む（readyでなければDataNotReady）
pub fn as7343_read_18ch<I: I2c>(i2c: &mut I) -> Result<[u16; 18], As7343Error<I::Error>> {
    if !as7343_data_ready(i2c)? {
        return Err(As7343Error::DataNotReady);
    }

    let mut raw = [0u8; 36];
    as7343_read_block(i2c, REG_DATA_0_L, &mut raw)?;

    let mut ch = [0u16; 18];
    for i in 0..18 {
        ch[i] = u16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]);
    }
    Ok(ch)
}
