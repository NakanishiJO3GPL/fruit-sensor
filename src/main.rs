#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod as7343;
mod xpt2046;

use panic_halt as _;

use core::cell::RefCell;

use display_interface_spi::SPIInterface;
use embassy_executor::Spawner;
use embassy_stm32::{
    gpio::{Input, Level, Output, Pull, Speed},
    i2c::{Config as I2cConfig, I2c},
    spi::{Config as SpiConfig, Spi},
    time::Hertz,
    usart::{Config as UartConfig, UartTx},
};
use embassy_time::Timer;
use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
};
use embedded_hal_bus::spi::RefCellDevice;
use ili9341::{Ili9341, Orientation};
use xpt2046::{Calibration, Xpt2046};

const DISPLAY_CHANNELS: [u8; 11] = [12, 6, 0, 7, 8, 15, 2, 9, 13, 14, 3];
const DISPLAY_LABELS: [&str; 11] = [
    "F1:", "F2:", "FZ:", "F3:", "F4:", "F5:", "FXL:", "F6:", "F7:", "F8:", "NIR:",
];

const GRAPH_X: i32 = 56;
const GRAPH_W: u32 = 248;
const ROW_Y0: i32 = 40;
const ROW_H: i32 = 14;

const LED_BTN_X: i32 = 170;
const LED_BTN_Y: i32 = 200;
const LED_BTN_W: u32 = 140;
const LED_BTN_H: u32 = 28;

const TOUCH_CAL: Calibration = Calibration {
    x_min: 0,
    x_max: 4095,
    y_min: 0,
    y_max: 4095,
    width: 320,
    height: 240,
    swap_xy: false,
    invert_x: false,
    invert_y: false,
};

// 実測された表示レンジ（未補正）
const TOUCH_X_MIN_MEASURED: u16 = 34;
const TOUCH_X_MAX_MEASURED: u16 = 285;
const TOUCH_Y_MIN_MEASURED: u16 = 26;
const TOUCH_Y_MAX_MEASURED: u16 = 216;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // Set max frequency (170MHz) to system clock
    let mut config = embassy_stm32::Config::default();
    config.rcc.sys = embassy_stm32::rcc::Sysclk::PLL1_R;
    config.rcc.pll = Some(embassy_stm32::rcc::Pll {
        source: embassy_stm32::rcc::PllSource::HSI,
        prediv: embassy_stm32::rcc::PllPreDiv::DIV4,
        mul: embassy_stm32::rcc::PllMul::MUL85,
        divp: None,
        divq: None,
        divr: Some(embassy_stm32::rcc::PllRDiv::DIV2),
    });
    config.rcc.ahb_pre = embassy_stm32::rcc::AHBPrescaler::DIV1;
    config.rcc.apb1_pre = embassy_stm32::rcc::APBPrescaler::DIV1;
    config.rcc.apb2_pre = embassy_stm32::rcc::APBPrescaler::DIV1;

    let p = embassy_stm32::init(config);

    spawner.spawn(
        as7343_lcd_task(
            p.I2C1,
            p.PA15,     // MUST Remove SB3 0ohm jumper
            p.PB7,      // MUST Remove SB2 0ohm jumper
            p.SPI1,
            p.PA5,
            p.PA7,
            p.PA6,
            p.PA4,
            p.PB0,
            p.PA12,
            p.PA0,
            p.PA1,
            p.USART2,
            p.PA2,
        )
        .unwrap(),
    );

    loop {
        Timer::after(embassy_time::Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn as7343_lcd_task(
    i2c1: embassy_stm32::Peri<'static, embassy_stm32::peripherals::I2C1>,
    i2c_scl: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PA15>,
    i2c_sda: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PB7>,
    spi1: embassy_stm32::Peri<'static, embassy_stm32::peripherals::SPI1>,
    spi_sck: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PA5>,
    spi_mosi: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PA7>,
    spi_miso: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PA6>,
    lcd_cs: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PA4>,
    touch_cs: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PB0>,
    touch_irq: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PA12>,
    lcd_dc: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PA0>,
    lcd_rst: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PA1>,
    uart2: embassy_stm32::Peri<'static, embassy_stm32::peripherals::USART2>,
    uart_tx: embassy_stm32::Peri<'static, embassy_stm32::peripherals::PA2>,
) {
    // NUCLEO ST-LINK Virtual COM Port (USART2 TX=PA2)
    let mut uart_cfg = UartConfig::default();
    uart_cfg.baudrate = 115_200;
    let mut log_uart = UartTx::new_blocking(uart2, uart_tx, uart_cfg).unwrap();
    let _ = log_uart.blocking_write(b"boot: nucleo vcp uart2\r\n");

    // SPI/LCD init
    let mut spi_config = SpiConfig::default();
    // XPT2046は高速SPIに弱いため、LCDと共有バス時は低めに設定
    spi_config.frequency = Hertz(2_000_000);
    let spi = Spi::new_blocking(spi1, spi_sck, spi_mosi, spi_miso, spi_config);
    let spi_bus = RefCell::new(spi);

    let cs1 = Output::new(lcd_cs, Level::High, Speed::VeryHigh);
    let cs2 = Output::new(touch_cs, Level::High, Speed::VeryHigh);
    let penirq = Input::new(touch_irq, Pull::Up);
    let dc = Output::new(lcd_dc, Level::Low, Speed::VeryHigh);
    let mut rst = Output::new(lcd_rst, Level::Low, Speed::High);
    rst.set_high();

    let spi_disp_device = RefCellDevice::new_no_delay(&spi_bus, cs1).unwrap();
    let spi_disp_iface = SPIInterface::new(spi_disp_device, dc);

    let spi_touch_device = RefCellDevice::new_no_delay(&spi_bus, cs2).unwrap();
    let mut touch = Xpt2046::new(spi_touch_device, penirq);

    let mut display = Ili9341::new(
        spi_disp_iface,
        rst,
        &mut embassy_time::Delay,
        Orientation::Landscape,
        ili9341::DisplaySize240x320,
    )
    .unwrap();

    // I2C init
    let mut i2c_config = I2cConfig::default();
    i2c_config.frequency = Hertz(100_000);
    let mut i2c = I2c::new_blocking(i2c1, i2c_scl, i2c_sda, i2c_config);

    // 初期化失敗時は再試行し、ログが壊れないよう固定ASCIIで出力
    loop {
        match as7343::as7343_init_default(&mut i2c) {
            Ok(()) => {
                let _ = log_uart.blocking_write(b"as7343 init ok\r\n");
                break;
            }
            Err(as7343::As7343Error::I2cWriteRead(_)) => {
                let _ = log_uart.blocking_write(b"as7343 init err: i2c-wr\r\n");
            }
            Err(as7343::As7343Error::I2cWrite(_)) => {
                let _ = log_uart.blocking_write(b"as7343 init err: i2c-w\r\n");
            }
            Err(as7343::As7343Error::I2cRead(_)) => {
                let _ = log_uart.blocking_write(b"as7343 init err: i2c-r\r\n");
            }
            Err(as7343::As7343Error::InvalidChipId(id)) => {
                let _ = log_uart.blocking_write(b"as7343 init err: id=");
                let mut idbuf = [0u8; 8];
                let idstr = u32_to_str(id as u32, &mut idbuf);
                let _ = log_uart.blocking_write(idstr.as_bytes());
                let _ = log_uart.blocking_write(b"\r\n");
            }
            Err(as7343::As7343Error::DataNotReady) => {
                let _ = log_uart.blocking_write(b"as7343 init err: not-ready\r\n");
            }
        }
        Timer::after(embassy_time::Duration::from_millis(500)).await;
    }

    let _ = as7343::as7343_set_led_enable(&mut i2c, false);

    display.clear(Rgb565::BLACK).unwrap();
    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

    // 固定要素は初回のみ描画（高速化）
    Text::new("AS7343 Spectrum", Point::new(8, 12), style)
        .draw(&mut display)
        .unwrap();
    for slot in 0..DISPLAY_CHANNELS.len() {
        let y = ROW_Y0 + (slot as i32) * ROW_H;

        Text::new(DISPLAY_LABELS[slot], Point::new(8, y), style)
            .draw(&mut display)
            .unwrap();

        // グラフ領域の枠（背景）
        Rectangle::new(Point::new(GRAPH_X, y - 8), Size::new(GRAPH_W, 8))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(2, 2, 2)))
            .draw(&mut display)
            .unwrap();
    }

    let mut led_on = false;
    let mut prev_touched = false;
    draw_led_button(&mut display, style, led_on);

    loop {
        match as7343::as7343_data_ready(&mut i2c) {
            Ok(true) => match as7343::as7343_read_18ch(&mut i2c) {
                Ok(ch) => {
                    // 値を横棒グラフとして更新（バー領域のみ）
                    for (slot, &ch_idx) in DISPLAY_CHANNELS.iter().enumerate() {
                        let y = ROW_Y0 + (slot as i32) * ROW_H;

                        // 前回バーをクリア
                        Rectangle::new(Point::new(GRAPH_X, y - 8), Size::new(GRAPH_W, 8))
                            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                            .draw(&mut display)
                            .unwrap();

                        let raw = ch[ch_idx as usize] as u32;
                        let mut w = (raw * GRAPH_W) / 65535;
                        if w == 0 && raw > 0 {
                            w = 1;
                        }

                        if w > 0 {
                            Rectangle::new(Point::new(GRAPH_X, y - 8), Size::new(w, 8))
                                .into_styled(PrimitiveStyle::with_fill(color_for_slot(slot)))
                                .draw(&mut display)
                                .unwrap();
                        }
                    }

                    // タッチ座標をUART/LCDへ表示
                    Rectangle::new(Point::new(208, 0), Size::new(112, 16))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(&mut display)
                        .unwrap();

                    if let Ok(is_touched_now) = touch.is_touched() {
                        if is_touched_now {
                            if let Ok(p_raw) = touch.read_point(&TOUCH_CAL) {
                                let p = normalize_touch_point(p_raw);

                                let _ = log_uart.blocking_write(b"touch x=");
                                let mut xbuf = [0u8; 8];
                                let mut ybuf = [0u8; 8];
                                let xs = u32_to_str(p.x as u32, &mut xbuf);
                                let _ = log_uart.blocking_write(xs.as_bytes());
                                let _ = log_uart.blocking_write(b",y=");
                                let ys = u32_to_str(p.y as u32, &mut ybuf);
                                let _ = log_uart.blocking_write(ys.as_bytes());
                                let _ = log_uart.blocking_write(b"\r\n");

                                Text::new("T:", Point::new(210, 12), style)
                                    .draw(&mut display)
                                    .unwrap();
                                Text::new(xs, Point::new(224, 12), style)
                                    .draw(&mut display)
                                    .unwrap();
                                Text::new(",", Point::new(252, 12), style)
                                    .draw(&mut display)
                                    .unwrap();
                                Text::new(ys, Point::new(258, 12), style)
                                    .draw(&mut display)
                                    .unwrap();

                                if !prev_touched && point_in_led_button(p) {
                                    led_on = !led_on;
                                    let _ = as7343::as7343_set_led_enable(&mut i2c, led_on);
                                    draw_led_button(&mut display, style, led_on);
                                    if led_on {
                                        let _ = log_uart.blocking_write(b"LED ON\r\n");
                                    } else {
                                        let _ = log_uart.blocking_write(b"LED OFF\r\n");
                                    }
                                }
                            }
                        } else {
                            Text::new("T:---,---", Point::new(210, 12), style)
                                .draw(&mut display)
                                .unwrap();
                        }

                        prev_touched = is_touched_now;
                    }
                }
                Err(_) => {
                    let _ = log_uart.blocking_write(b"as7343 read err\r\n");
                }
            },
            Ok(false) => {}
            Err(_) => {
                let _ = log_uart.blocking_write(b"as7343 ready err\r\n");
            }
        }

        Timer::after(embassy_time::Duration::from_millis(100)).await;
    }
}


fn point_in_led_button(p: xpt2046::Point) -> bool {
    let x = p.x as i32;
    let y = p.y as i32;
    x >= LED_BTN_X
        && x < LED_BTN_X + LED_BTN_W as i32
        && y >= LED_BTN_Y
        && y < LED_BTN_Y + LED_BTN_H as i32
}

fn draw_led_button<D>(display: &mut D, style: MonoTextStyle<'_, Rgb565>, led_on: bool)
where
    D: DrawTarget<Color = Rgb565>,
{
    let fill = if led_on {
        Rgb565::new(0, 40, 0)
    } else {
        Rgb565::new(20, 0, 0)
    };

    Rectangle::new(Point::new(LED_BTN_X, LED_BTN_Y), Size::new(LED_BTN_W, LED_BTN_H))
        .into_styled(PrimitiveStyle::with_fill(fill))
        .draw(display)
        .ok();

    let label = if led_on { "LED: ON" } else { "LED: OFF" };
    Text::new(label, Point::new(LED_BTN_X + 10, LED_BTN_Y + 18), style)
        .draw(display)
        .ok();
}

fn normalize_touch_point(p: xpt2046::Point) -> xpt2046::Point {
    let x = map_range_clamped(p.x, TOUCH_X_MIN_MEASURED, TOUCH_X_MAX_MEASURED, 320);
    let y = map_range_clamped(p.y, TOUCH_Y_MIN_MEASURED, TOUCH_Y_MAX_MEASURED, 240);

    xpt2046::Point { x, y: 239 - y }
}

fn map_range_clamped(v: u16, in_min: u16, in_max: u16, out_max: u16) -> u16 {
    if in_max <= in_min || out_max == 0 {
        return 0;
    }

    let c = v.clamp(in_min, in_max);
    let num = (c - in_min) as u32 * (out_max as u32 - 1);
    let den = (in_max - in_min) as u32;
    (num / den) as u16
}

fn color_for_slot(slot: usize) -> Rgb565 {
    match slot {
        0 => Rgb565::new(20, 0, 20),  // F1: 紫
        1 => Rgb565::new(0, 0, 16),   // F2: 濃い青
        2 => Rgb565::new(0, 0, 31),   // FZ: 青
        3 => Rgb565::new(0, 63, 31),  // F3: 水色
        4 => Rgb565::new(0, 40, 20),  // F4: 緑と水色の間
        5 => Rgb565::new(18, 63, 0),  // F5: 黄緑
        6 => Rgb565::new(31, 32, 0),  // FXL: 橙
        7 => Rgb565::new(31, 16, 0),  // F6: 赤よりの橙
        8 => Rgb565::new(31, 0, 0),   // F7: 赤
        9 => Rgb565::new(28, 16, 0),   // F8: 焦げ茶
        _ => Rgb565::WHITE,           // NIR: 白
    }
}

fn u32_to_str(mut num: u32, buffer: &mut [u8]) -> &str {
    if num == 0 {
        buffer[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buffer[0..1]) };
    }
    let mut i = 0;
    while num > 0 {
        buffer[i] = b'0' + (num % 10) as u8;
        num /= 10;
        i += 1;
    }

    buffer[0..i].reverse();
    unsafe { core::str::from_utf8_unchecked(&buffer[0..i]) }
}
