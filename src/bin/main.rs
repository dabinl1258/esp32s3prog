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
use core::convert::Infallible;
use core::fmt::Write;
use core::future::ready;
use defmt_rtt as _;

//use core::fmt::DebugList;
//use embedded_hal::delay::DelayNs;
use esp_hal::delay::Delay;

use defmt::info;

use embedded_cli::Command;
use embedded_cli::cli::CliBuilder;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Flex, Level, Output, OutputConfig};
use esp_hal::main;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_println as _;

const MEM_SIZE: usize = 32768usize;
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
            time_clk_low: 200,
            time_clk_high: 200,
            time_dummy_low: 200,
            time_dummy_high: 200,
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

    pub fn read(&mut self, addr: u16, len: usize) -> [u8; MEM_SIZE] {
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

        let mut mem: [u8; MEM_SIZE] = [0; MEM_SIZE];
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
    pub fn write_all(&mut self, addr: usize, bytes: [u8; MEM_SIZE], len: usize) {
        self.start_condition();
        self.send_byte(0x00);
        self.send_byte((addr >> 8) as u8);
        self.send_byte(addr as u8);

        for idx in 0..len {
            let byte = bytes[idx];
            self.send_byte(byte);
        }

        self.send_byte(0xFF);
        self.stop_condition();
    }
    pub fn write_smart_option(&mut self, addr: usize, byte: u8) {
        self.start_condition();
        // 0b1110 0000
        // 0x E   0
        self.send_byte(0xE0);
        self.send_byte((addr >> 8) as u8);
        self.send_byte(addr as u8);
        self.send_byte(byte);
        self.send_byte(0xFF);

        self.sclk.set_high();
        self.delay.delay_millis(30);
        self.stop_condition();
        self.delay.delay_millis(20);
    }
    pub fn read_smart_option(&mut self, addr: usize) -> u8 {
        self.start_condition();
        // 0b1110 0001
        // 0x E   1
        self.send_byte(0xE1);
        self.send_byte((addr >> 8) as u8);
        self.send_byte(addr as u8);
        let value = self.read_byte();
        self.stop_condition();
        value
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
#[derive(Command, Debug)]
enum BaseCommand {
    Mem {
        addr: u16,
        len: usize,
    },
    Init,
    Reset,
    Write {
        addr: u16,
        byte: u8,
    },
    Crc,
    Upload {
        size: usize,
    },
    Download,
    Flash {
        addr: usize,
        size: usize,
    },
    Erase,
    Readbyte {
        addr: u16,
        len: usize,
    },
    Status,
    /// 두 수의 합 계산 (테스트용)
    Add {
        a: i32,
        b: i32,
    },
    Auto,
    Verify {
        addr: usize,
        size: usize,
    },
    Smart,
}

#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let _peripherals = esp_hal::init(config);
    esp_println::logger::init_logger_from_env();
    // esp_println::logger::init_logger();
    esp_println::println!("test");

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
    let (mut rx, mut tx) = usb_serial.split();
    let delay = Delay::new();

    let len = 128;

    let mut s3 = S3interface::new(reset, vpp, sclk, sdat);

    let mut command_buffer = [0u8; 550];
    let mut history_buffer = [0u8; 550];
    let mut flash_buffer = [0u8; MEM_SIZE];

    let mut cli = CliBuilder::default()
        .writer(&mut tx)
        .command_buffer(command_buffer)
        .history_buffer(history_buffer)
        .build()
        .ok()
        .unwrap();

    loop {
        // USB로부터 바이트를 꺼내서 CLI 프로세서에 전달

        while let Ok(byte) = rx.read_byte() {
            let _ = cli.process_byte::<BaseCommand, _>(
                byte,
                &mut BaseCommand::processor(|cli, command| {
                    match command {
                        BaseCommand::Erase => {
                            s3.init();
                            s3.enter_program_mode();
                            s3.erase();
                            s3.init();
                        }
                        BaseCommand::Init => {
                            s3.init();
                        }
                        BaseCommand::Reset => {
                            writeln!(cli.writer(), "reset").ok();
                        }
                        BaseCommand::Write { addr, byte } => {
                            s3.init();
                            s3.enter_program_mode();
                            s3.write(addr, byte);
                            s3.stop_condition();
                            writeln!(cli.writer(), "write").ok();
                        }
                        BaseCommand::Readbyte { addr, len } => {
                            s3.init();
                            s3.enter_program_mode();
                            s3.start_condition();
                            let mem = s3.read(addr, len);
                            s3.stop_condition();

                            for idx in 0..len {
                                writeln!(cli.writer(), "{}", mem[idx]).ok();
                            }

                            writeln!(cli.writer(), "read").ok();
                        }

                        BaseCommand::Smart {} => {
                            s3.init();
                            s3.enter_program_mode();
                            s3.write_smart_option(0x0E39, 0xA7);
                            delay.delay_millis(10);
                            s3.init();
                            s3.enter_program_mode();

                            let smart_option = s3.read_smart_option(0x0E39);
                            if smart_option == 0xA7 {
                                writeln!(cli.writer(), "smart option yes").ok();
                            } else {
                                writeln!(cli.writer(), "smart option no{}", smart_option).ok();
                            }
                        }
                        BaseCommand::Mem { addr, len } => {
                            let addr = addr as usize;
                            for idx in addr..(addr + len) {
                                writeln!(cli.writer(), "{}", flash_buffer[idx]).ok();
                            }

                            writeln!(cli.writer(), "read").ok();
                        }
                        BaseCommand::Status => {
                            writeln!(cli.writer(), "Native USB CDC CLI 정상 작동 중! 🐻").ok();
                        }

                        BaseCommand::Add { a, b } => {
                            writeln!(cli.writer(), "계산 결과: {} + {} = {}", a, b, a + b).ok();
                        }
                        BaseCommand::Auto => {
                            s3.init();
                            s3.enter_program_mode();

                            s3.erase();
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

                                for idx in 0..len {
                                    writeln!(cli.writer(), "{}", mem[idx]).ok();
                                }
                                //writeln!(cli.writer(), "{}", mem[0..128]).ok();
                                s3.stop_condition();
                            }
                        }
                        BaseCommand::Upload { size } => {
                            let mut idx = 0usize;
                            loop {
                                let byte = rx.read_byte();
                                match byte {
                                    Ok(byte) => {
                                        flash_buffer[idx] = byte;
                                        idx += 1;
                                        if idx > MEM_SIZE {
                                            break;
                                        }
                                        if idx >= size {
                                            break;
                                        }
                                    }
                                    Err(_) => {
                                        delay.delay_micros(1);
                                    }
                                }
                            }
                        }
                        BaseCommand::Flash { addr, size } => {
                            s3.init();
                            s3.enter_program_mode();
                            s3.write_all(addr, flash_buffer, size);
                            s3.init();
                            s3.enter_program_mode();
                            s3.write_smart_option(0x0E39, 0xA7); //(Internal RC 8MHz, LVR Enable & Level = 2.3V)
                        }
                        BaseCommand::Download => {}
                        BaseCommand::Crc => {}
                        BaseCommand::Verify { addr, size } => {
                            s3.init();
                            s3.enter_program_mode();
                            let addr = addr as u16;
                            s3.start_condition();
                            let read = s3.read(addr, size);
                            let smart_option = s3.read_smart_option(0x0E39);
                            if flash_buffer[0..size] == read[0..size] && smart_option == 0xA7 {
                                writeln!(cli.writer(), "verify ok").ok();
                            } else {
                                writeln!(cli.writer(), "verify false").ok();
                            }
                        }
                    }
                    Ok::<(), Infallible>(())
                }),
            );
        }

        delay.delay_millis(10);
    }
    // s3.init();
    // s3.enter_program_mode();

    // //    s3.erase();
    // s3.init();
    // s3.enter_program_mode();

    // for addr in 0..100 {
    //     s3.init();
    //     s3.enter_program_mode();
    //     s3.write(addr, 0xaa);
    // }
    // for addr in 0..1 {
    //     s3.init();
    //     s3.enter_program_mode();
    //     s3.start_condition();
    //     let mem = s3.read(addr, len);
    //     info!("{}", mem[0..128]);
    //     s3.stop_condition();
    // }
    // s3.init();
    // s3.enter_program_mode();
    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.1.0/examples
}
