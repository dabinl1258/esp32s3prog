#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
//use core::fmt::DebugList;
//use embedded_hal::delay::DelayNs;
use esp_hal::delay::Delay;

use defmt::info;

use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Flex, Level, Output, OutputConfig};
use esp_hal::main;
use esp_hal::time::{Duration, Instant};
use esp_println as _;

struct S3interface<'a> {
    reset: Output<'a>,
    vpp: Output<'a>,
    sclk: Output<'a>,
    sdat: Flex<'a>,

    time_setup_start: u32,
    time_setup_hold: u32,
    time_clk_low: u32,
    time_clk_high: u32,
    delay: Delay,
}

impl<'a> S3interface<'a> {
    pub fn new(reset: Output<'a>, vpp: Output<'a>, sclk: Output<'a>, sdat: Flex<'a>) -> Self {
        Self {
            reset,
            vpp,
            sclk,
            sdat,
            time_setup_start: 100,
            time_setup_hold: 100,
            time_clk_low: 100,
            time_clk_high: 100,
            delay: Delay::new(),
        }
    }

    pub fn init(&mut self) {
        self.reset.set_high();
        self.vpp.set_low();
        self.sclk.set_low();

        self.sdat.set_input_enable(false);
        self.sdat.set_output_enable(true);
        self.sdat.set_low();
    }

    /// 프로그래밍 모드 시작 함수
    pub fn enterProgramMode(&mut self) {
        self.reset.set_low();
        self.vpp.set_high();
    }

    pub fn dummyClk(&mut self) {
        self.sdat.set_input_enable(false);
        self.sdat.set_output_enable(true);

        self.delay.delay_micros(self.time_clk_low);
        self.sdat.set_high();
        self.sclk.set_high();
        self.delay.delay_micros(self.time_clk_high);
        self.sclk.set_low();
        self.sdat.set_low();

        self.sdat.set_input_enable(true);
        self.sdat.set_output_enable(false);
    }

    pub fn read(&mut self, addr: u16, len: usize) -> [u8; 128] {
        let byte1: u8 = 0x61u8;
        for i in (0..=7).rev() {
            //info!("i : {}", i);
            if ((0x01 << i) & byte1) != 0 {
                self.sdat.set_high();
            } else {
                self.sdat.set_low();
            }

            self.delay.delay_micros(self.time_clk_low);
            self.sclk.set_high();
            self.delay.delay_micros(self.time_clk_high);
            self.sclk.set_low();
        }
        self.dummyClk();
        for i in (0..=15).rev() {
            if ((0x01u16 << i) & addr) != 0 {
                self.sdat.set_high();
            } else {
                self.sdat.set_low();
            }

            self.delay.delay_micros(self.time_clk_low);
            self.sclk.set_high();
            self.delay.delay_micros(self.time_clk_high);
            self.sclk.set_low();
            if i == 8 || i == 0 {
                self.dummyClk();
            }
        }

        let mut mem: [u8; 128] = [0; 128];
        for i in 0..len {
            mem[i] = self.readbyte();
        }
        mem
    }
    pub fn readbyte(&mut self) -> u8 {
        let mut readed: u8 = 0u8;
        for i in (0..=7).rev() {
            self.delay.delay_micros(self.time_clk_low);
            self.sclk.set_high();
            self.delay.delay_micros(self.time_clk_high / 2);
            if self.sdat.is_high() {
                readed = readed | (0x01 << i);
            }
            self.delay.delay_micros(self.time_clk_high / 2);
            self.sclk.set_low();
        }
        readed
    }
    pub fn startCondition(&mut self) {
        self.sdat.set_input_enable(false);
        self.sdat.set_output_enable(true);
        self.sclk.set_high();
        self.delay.delay_nanos(self.time_setup_start);
        self.sdat.set_high();
        self.delay.delay_nanos(self.time_setup_hold);
    }
    pub fn stopCondition(&mut self) {
        self.sdat.set_input_enable(false);
        self.sdat.set_output_enable(true);
    }
}

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let _peripherals = esp_hal::init(config);
    let mut reset = Output::new(
        _peripherals.GPIO4,
        Level::High,
        OutputConfig::default().with_drive_mode(esp_hal::gpio::DriveMode::PushPull),
    );

    let mut vpp = Output::new(
        _peripherals.GPIO5,
        Level::Low,
        OutputConfig::default().with_drive_mode(esp_hal::gpio::DriveMode::PushPull),
    );
    let mut sclk = Output::new(
        _peripherals.GPIO7,
        Level::Low,
        OutputConfig::default().with_drive_mode(esp_hal::gpio::DriveMode::PushPull),
    );
    let mut sdat = Flex::new(_peripherals.GPIO6);
    sdat.set_input_enable(false);
    sdat.set_output_enable(true);
    sdat.set_low();

    let delay = Delay::new();
    let time_setup_start: u32 = 1000; // ns
    let time_setup_hold: u32 = 150; // ns
    let time_clk_low: u32 = 200; // us
    let time_clk_high: u32 = 200; // us

    let byte1: u8 = 0x61;
    let mut addr: u16 = 128;
    let mut readed: u8;
    let mut mem: [u8; 128] = [0; 128];
    info!("Hello");
    info!("READ {} to {}", addr, addr + 127);
    reset.set_high();
    vpp.set_low();

    info!("start pattern");
    reset.set_low();
    delay.delay_nanos(100);
    vpp.set_high();
    loop {
        sclk.set_high();
        delay.delay_nanos(time_setup_start);
        sdat.set_high();
        delay.delay_nanos(time_setup_hold);
        // now start pattern
        sclk.set_low();

        sdat.set_low();
        sdat.set_input_enable(false);
        sdat.set_output_enable(true);

        for i in (0..=7).rev() {
            //info!("i : {}", i);
            if ((0x01 << i) & byte1) != 0 {
                sdat.set_high();
            } else {
                sdat.set_low();
            }

            delay.delay_micros(time_clk_low);
            sclk.set_high();
            delay.delay_micros(time_clk_high);
            sclk.set_low();
        }
        for i in (0..=15).rev() {
            if ((0x01u16 << i) & addr) != 0 {
                sdat.set_high();
            } else {
                sdat.set_low();
            }

            delay.delay_micros(time_clk_low);
            sclk.set_high();
            delay.delay_micros(time_clk_high);
            sclk.set_low();
        }
        // dummy clock for read
        //
        //
        //
        //let mut cnt = 0;

        for j in 0..=127usize {
            sdat.set_input_enable(false);
            sdat.set_output_enable(true);

            delay.delay_micros(time_clk_low);
            sdat.set_high();
            sclk.set_high();
            delay.delay_micros(time_clk_high);
            sclk.set_low();
            sdat.set_low();
            delay.delay_micros(time_clk_low);

            sdat.set_input_enable(true);
            sdat.set_output_enable(false);

            readed = 0u8;

            for i in (0..=7).rev() {
                delay.delay_micros(time_clk_low);
                sclk.set_high();
                delay.delay_micros(time_clk_high / 2);
                if sdat.is_high() {
                    readed = readed | (0x01 << i);
                }
                delay.delay_micros(time_clk_high / 2);
                sclk.set_low();
            }
            //info!("{}", readed);
            mem[j] = readed;
            delay.delay_micros(40);
            //info!("0x{} + {} result {}", addr, j, readed);
        }

        for j in 0..=127usize {
            info!("{}", mem[j]);
        }
        info!("Done");
        loop {
            let delay_start = Instant::now();
            while delay_start.elapsed() < Duration::from_millis(500) {}
            break;
        }
        break;
    }
    let mut s3 = S3interface::new(reset, vpp, sclk, sdat);
    s3.init();
    s3.enterProgramMode();
    loop {}

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.1.0/examples
}
