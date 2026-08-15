#![no_std]
#![no_main]

use panic_halt as _;

use core::cell::RefCell;

use display_interface_spi::SPIInterface;
use embassy_executor::Spawner;
use embassy_stm32::{
    bind_interrupts,
    exti::{self, ExtiInput},
    gpio::{Level, Output, Pull, Speed},
    interrupt,
    mode::Async,
    spi::{Config as SpiConfig, Spi},
    time::Hertz,
};
use embassy_time::Timer;
use embedded_graphics::{
    mono_font::{jis_x0201::FONT_10X20, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
};
use embedded_hal_bus::spi::RefCellDevice;
use ili9341::{Ili9341, Orientation};

bind_interrupts!(
    pub struct Irqs{
        EXTI2 => exti::InterruptHandler<interrupt::typelevel::EXTI2>;
    }
);

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

    /* SPI1:
       SCK  PA5(CN3-8pin)   ... LCD 7pin / Touch 10pin
       MISO PA6(CN3-7pin)   ... LCD ---- / Touch 13pin
       MOSI PA7(CN3-6pin)   ... LCD 6pin / Touch 12pin
       CS1  PA4(CN3-9pin)   ... LCD 3pin / Touch -----
       CS2  PA3(CN3-10pin)  ... LCD ---- / Touch 11pin
       DC   PA0(CN3-12pin)  ... LCD 5pin
       RST  PA1(CN3-11pin)  ... LCD 4pin
       INTR PA2(CN3-5pin)   ... LCD ---- / Touch 14pin
    */
    let mut spi_config = SpiConfig::default();
    spi_config.frequency = Hertz(84_000_000);
    let spi = Spi::new_blocking(p.SPI1, p.PA5, p.PA7, p.PA6, spi_config);
    let spi_bus = RefCell::new(spi);

    let cs1 = Output::new(p.PA4, Level::High, Speed::VeryHigh);
    let cs2 = Output::new(p.PA3, Level::High, Speed::VeryHigh);
    let dc = Output::new(p.PA0, Level::Low, Speed::VeryHigh);
    let mut rst = Output::new(p.PA1, Level::Low, Speed::High);
    let mut intr = ExtiInput::new(p.PA2, p.EXTI2, Pull::None, Irqs);

    // Reset control
    rst.set_high();

    // Display SPI interface
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

    // Touch SPI interface
    let spi_touch_device = RefCellDevice::new_no_delay(&spi_bus, cs2).unwrap();

    // Display
    display.clear(Rgb565::BLACK).unwrap();
    Rectangle::new(Point::new(100, 150), Size::new(30, 30))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::RED))
        .draw(&mut display)
        .unwrap();

    let sysclk = embassy_stm32::rcc::frequency::<embassy_stm32::peripherals::SYSCFG>().0;

    let style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    Text::new("SYSCLK:", Point::new(20, 40), style)
        .draw(&mut display)
        .unwrap();
    let mut buf = [0u8; 12];
    let mhz = sysclk / 1_000_000;
    let mhz_str = u32_to_str(mhz, &mut buf);
    Text::new(mhz_str, Point::new(100, 40), style)
        .draw(&mut display)
        .unwrap();

    Text::new("MHz", Point::new(140, 40), style)
        .draw(&mut display)
        .unwrap();

    spawner.spawn(touch_waiter(intr).unwrap());

    loop {
        // display を使った処理をここに記述
        Timer::after(embassy_time::Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn touch_waiter(mut intr: ExtiInput<'static, Async>) {
    intr.wait_for_falling_edge().await;
    loop {
        // dummy
        Timer::after(embassy_time::Duration::from_secs(1)).await;
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
