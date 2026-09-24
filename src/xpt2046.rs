use embedded_hal::{digital::InputPin, spi::{Operation, SpiDevice}};

const CMD_X: u8 = 0xD0; // 12-bit, differential, X position
const CMD_Y: u8 = 0x90; // 12-bit, differential, Y position

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawPoint {
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct Calibration {
    pub x_min: u16,
    pub x_max: u16,
    pub y_min: u16,
    pub y_max: u16,
    pub width: u16,
    pub height: u16,
    pub swap_xy: bool,
    pub invert_x: bool,
    pub invert_y: bool,
}

#[derive(Debug)]
pub enum Error<SpiE, PinE> {
    Spi(SpiE),
    Pin(PinE),
    InvalidCalibration,
}

pub struct Xpt2046<SPI, IRQ> {
    spi: SPI,
    irq: IRQ,
}

impl<SPI, IRQ> Xpt2046<SPI, IRQ> {
    pub fn new(spi: SPI, irq: IRQ) -> Self {
        Self { spi, irq }
    }
}

impl<SPI, IRQ, SpiE, PinE> Xpt2046<SPI, IRQ>
where
    SPI: SpiDevice<Error = SpiE>,
    IRQ: InputPin<Error = PinE>,
{
    pub fn is_touched(&mut self) -> Result<bool, Error<SpiE, PinE>> {
        // XPT2046のPENIRQは通常アクティブLow
        self.irq.is_low().map_err(Error::Pin)
    }

    pub fn read_raw(&mut self) -> Result<RawPoint, Error<SpiE, PinE>> {
        let x = self.read_channel(CMD_X)?;
        let y = self.read_channel(CMD_Y)?;
        Ok(RawPoint { x, y })
    }

    pub fn read_raw_averaged(&mut self, samples: u8) -> Result<RawPoint, Error<SpiE, PinE>> {
        let n = if samples == 0 { 1 } else { samples.min(16) } as u32;
        let mut sx = 0u32;
        let mut sy = 0u32;

        for _ in 0..n {
            let p = self.read_raw()?;
            sx += p.x as u32;
            sy += p.y as u32;
        }

        Ok(RawPoint {
            x: (sx / n) as u16,
            y: (sy / n) as u16,
        })
    }

    pub fn read_point(&mut self, cal: &Calibration) -> Result<Point, Error<SpiE, PinE>> {
        let raw = self.read_raw_averaged(4)?;
        map_raw_to_screen(raw, cal)
    }

    fn read_channel(&mut self, cmd: u8) -> Result<u16, Error<SpiE, PinE>> {
        // 1バイト目で制御コマンド送信、続く2バイトで測定値取得
        let mut buf = [cmd, 0, 0];
        self.spi
            .transaction(&mut [Operation::TransferInPlace(&mut buf)])
            .map_err(Error::Spi)?;

        // XPT2046の12bit値を抽出
        let v = (((buf[1] as u16) << 8) | (buf[2] as u16)) >> 3;
        Ok(v & 0x0FFF)
    }
}

fn map_raw_to_screen<SpiE, PinE>(
    raw: RawPoint,
    cal: &Calibration,
) -> Result<Point, Error<SpiE, PinE>> {
    if cal.x_max <= cal.x_min || cal.y_max <= cal.y_min || cal.width == 0 || cal.height == 0 {
        return Err(Error::InvalidCalibration);
    }

    let mut x = raw.x;
    let mut y = raw.y;

    if cal.swap_xy {
        core::mem::swap(&mut x, &mut y);
    }

    let x = clamp_map(x, cal.x_min, cal.x_max, cal.width);
    let y = clamp_map(y, cal.y_min, cal.y_max, cal.height);

    let x = if cal.invert_x { cal.width - 1 - x } else { x };
    let y = if cal.invert_y { cal.height - 1 - y } else { y };

    Ok(Point { x, y })
}

fn clamp_map(v: u16, in_min: u16, in_max: u16, out_max: u16) -> u16 {
    let c = v.clamp(in_min, in_max);
    let num = (c - in_min) as u32 * (out_max - 1) as u32;
    let den = (in_max - in_min) as u32;
    (num / den) as u16
}
