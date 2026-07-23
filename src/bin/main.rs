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
use core::fmt::Write;

//use core::fmt::DebugList;
//use embedded_hal::delay::DelayNs;
use esp_hal::delay::Delay;

use defmt::info;

use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Flex, Level, Output, OutputConfig};
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::{main, usb_serial_jtag};
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
    time_dummy_low: u32,
    time_dummy_high: u32,
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
            time_dummy_low: 100,
            time_dummy_high: 100,
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
    pub fn enter_program_mode(&mut self) {
        self.reset.set_low();
        self.delay.delay_micros(self.time_setup_start);
        self.vpp.set_high();
        self.delay.delay_micros(self.time_setup_hold);
    }

    pub fn dummy_clock(&mut self) {
        self.sdat.set_input_enable(false);
        self.sdat.set_output_enable(true);
        self.sclk.set_low();
        self.delay.delay_micros(1); // 핀 상태 안정화
        self.sdat.set_high();
        self.sclk.set_low();
        self.delay.delay_micros(self.time_dummy_low);

        self.sclk.set_high();
        self.delay.delay_micros(self.time_dummy_high);
    }

    pub fn read(&mut self, addr: u16, len: usize) -> [u8; 1024] {
        let byte1: u8 = 0x61u8;
        self.send_byte(byte1);
        self.send_byte((addr >> 8) as u8);
        self.send_byte(addr as u8);
        /*
        for i in (0..=7).rev() {
            self.sclk.set_low();
            if ((0x01 << i) & byte1) != 0 {
                self.sdat.set_high();
            } else {
                self.sdat.set_low();
            }

            self.delay.delay_micros(self.time_clk_low);
            self.sclk.set_high();
            self.delay.delay_micros(self.time_clk_high);
        }*/
        //self.dummy_clock();
        /*for i in (0..=15).rev() {
            self.sclk.set_low();
            if ((0x01u16 << i) & addr) != 0 {
                self.sdat.set_high();
            } else {
                self.sdat.set_low();
            }

            self.delay.delay_micros(self.time_dummy_low);
            self.sclk.set_high();
            self.delay.delay_micros(self.time_dummy_high);
            if i == 8 || i == 0 {
                self.dummy_clock();
            }
        }*/

        let mut mem: [u8; 1024] = [0; 1024];
        for i in 0..len {
            mem[i] = self.read_byte();
            //info!("{}", mem[i]);
        }
        mem
    }
    pub fn read_byte(&mut self) -> u8 {
        self.sdat.set_input_enable(true);
        self.sdat.set_output_enable(false);
        let mut readed: u8 = 0u8;
        for i in (0..=7).rev() {
            self.sclk.set_low();
            self.delay.delay_micros(self.time_clk_low);
            self.sclk.set_high();
            self.delay.delay_micros(self.time_clk_high / 2);
            if self.sdat.is_high() {
                //info!("test");
                readed = readed | (0x01 << i);
            }
            self.delay.delay_micros(self.time_clk_high / 2);
        }
        self.dummy_clock();
        readed
    }
    pub fn send_byte(&mut self, byte: u8) {
        self.sdat.set_input_enable(false);
        self.sdat.set_output_enable(true);
        for i in (0..=7).rev() {
            self.sclk.set_low();
            if ((0x01 << i) & byte) != 0 {
                self.sdat.set_high();
            } else {
                self.sdat.set_low();
            }

            self.delay.delay_micros(self.time_clk_low);
            self.sclk.set_high();
            self.delay.delay_micros(self.time_clk_high);
        }
        self.dummy_clock();
    }

    pub fn write(&mut self, addr: u16, byte: u8) {
        self.start_condition();
        self.send_byte(0x00);
        self.send_byte((addr >> 8) as u8);
        self.send_byte(addr as u8);
        self.send_byte(byte);
        self.send_byte(0xFF);
        self.stop_condition();
    }
    pub fn erase(&mut self) {
        self.start_condition();
        self.send_byte(0xE0);
        self.send_byte(0x55);
        self.send_byte(0x15);
        self.send_byte(0xAA);
        self.send_byte(0xFF);
        self.stop_condition();
        self.delay.delay_millis(2000);
    }
    pub fn start_condition(&mut self) {
        self.sdat.set_input_enable(false);
        self.sdat.set_output_enable(true);
        self.sdat.set_low();
        self.sclk.set_low();
        self.delay.delay_millis(1);

        self.sclk.set_high();
        self.delay.delay_nanos(self.time_setup_start);
        self.sdat.set_high();
        self.delay.delay_nanos(self.time_setup_hold);
    }
    pub fn stop_condition(&mut self) {
        self.sdat.set_input_enable(false);
        self.sdat.set_output_enable(true); // 마스터가 SDAT 제어권 확보
        self.sdat.set_high(); // SDAT를 High로 설정
        self.delay.delay_micros(1); // 안정화 대기
        self.sclk.set_high(); // SCLK를 High(VDD)로 유지
        self.delay.delay_micros(1); // thp 지연 (최소 1us) [7, 8]
        self.sdat.set_low(); // 하강 에지 발생 (Stop Condition) [1, 2]
        self.delay.delay_micros(1); // 종료 후 대기
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
    let reset = Output::new(
        _peripherals.GPIO4,
        Level::High,
        OutputConfig::default().with_drive_mode(esp_hal::gpio::DriveMode::PushPull),
    );

    let vpp = Output::new(
        _peripherals.GPIO5,
        Level::Low,
        OutputConfig::default().with_drive_mode(esp_hal::gpio::DriveMode::PushPull),
    );
    let sclk = Output::new(
        _peripherals.GPIO7,
        Level::Low,
        OutputConfig::default().with_drive_mode(esp_hal::gpio::DriveMode::PushPull),
    );
    let mut sdat = Flex::new(_peripherals.GPIO6);
    sdat.set_input_enable(false);
    sdat.set_output_enable(true);
    sdat.set_low();

    let mut usb_serial = UsbSerialJtag::new(_peripherals.USB_DEVICE);
    info!("Hello ");

    let mut rx_buf = [0u8; 64];

    loop {
        // 3. 수신 (Read): USB 버퍼에서 데이터 읽어오기 (Non-blocking)
        let byte = usb_serial.read_byte();

        match byte {
            Ok(T) => {
                usb_serial.write_char('d');
                if (T as char) == 'z' {
                    info!("im out");
                    break;
                }
            }
            Ok(127) => {
                break;
            }
            Err(E) => {

                // 수신 데이터가 없으면 루프 돌며 대기
            }
        }
    }

    let delay = Delay::new();
    info!("Hello");

    let len = 128;

    let mut s3 = S3interface::new(reset, vpp, sclk, sdat);
    s3.init();
    s3.enter_program_mode();

    //    s3.erase();
    s3.init();
    s3.enter_program_mode();

    for addr in 0..100 {
        s3.init();
        s3.enter_program_mode();
        s3.write(addr, 0xaa);
    }
    for addr in 0..1 {
        s3.init();
        s3.enter_program_mode();
        s3.start_condition();
        let mem = s3.read(addr, len);
        info!("{}", mem[0..128]);
        s3.stop_condition();
    }

    s3.init();
    s3.enter_program_mode();
    loop {
        delay.delay_millis(100);
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.1.0/examples
}
