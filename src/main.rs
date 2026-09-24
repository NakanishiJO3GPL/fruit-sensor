#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod as7343;

use panic_halt as _;

use core::cell::RefCell;

use display_interface_spi::SPIInterface;
use embassy_executor::Spawner;
use embassy_stm32::{
    gpio::{Level, Output, Speed},
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

const DISPLAY_CHANNELS: [u8; 11] = [12, 6, 0, 7, 8, 15, 2, 9, 13, 14, 3];
const DISPLAY_LABELS: [&str; 11] = [
    "F1:", "F2:", "FZ:", "F3:", "F4:", "F5:", "FXL:", "F6:", "F7:", "F8:", "NIR:",
];

const GRAPH_X: i32 = 56;
const GRAPH_W: u32 = 248;
const ROW_Y0: i32 = 40;
const ROW_H: i32 = 14;

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
    spi_config.frequency = Hertz(84_000_000);
    let spi = Spi::new_blocking(spi1, spi_sck, spi_mosi, spi_miso, spi_config);
    let spi_bus = RefCell::new(spi);

    let cs1 = Output::new(lcd_cs, Level::High, Speed::VeryHigh);
    let dc = Output::new(lcd_dc, Level::Low, Speed::VeryHigh);
    let mut rst = Output::new(lcd_rst, Level::Low, Speed::High);
    rst.set_high();

    let spi_disp_device = RefCellDevice::new_no_delay(&spi_bus, cs1).unwrap();
    let spi_disp_iface = SPIInterface::new(spi_disp_device, dc);

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
        9 => Rgb565::new(15, 8, 0),   // F8: 焦げ茶
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
