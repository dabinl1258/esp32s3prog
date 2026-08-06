#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]
extern crate alloc;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;

use esp_alloc as _;
//use core::fmt::DebugList;
//use embedded_hal::delay::DelayNs;
use defmt::info;
use embassy_net::{
    Runner, StackResources,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Flex, Level, Output, OutputConfig};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::ram;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_println as _;
use esp_println::println;
use esp_radio::wifi::{
    Config, ControllerConfig, Interface, WifiController, scan::ScanConfig, sta::StationConfig,
};
const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

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
#[cfg(feature = "alloc-hooks")]
#[unsafe(no_mangle)]
unsafe extern "Rust" fn _esp_alloc_alloc(
    _heap: &EspHeap,
    _caps: EnumSet<MemoryCapability>,
    ptr: usize,
    size: usize,
) {
    println!("Allocated {} bytes: {:x}", size, ptr);
}

#[cfg(feature = "alloc-hooks")]
#[unsafe(no_mangle)]
unsafe extern "Rust" fn _esp_alloc_dealloc(_heap: &EspHeap, ptr: usize, size: usize) {
    println!("Deallocated {} bytes: {:x}", size, ptr);
}

// #[allow(
//     clippy::large_stack_frames,
//     reason = "it's not unusual to allocate larger buffers etc. in main"
// )]
//#[main]
//[esp_hal_embassy::main]
#[esp_rtos::main]
async fn main(spawner: Spawner) {
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 64 * 1024);
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let _peripherals = esp_hal::init(config);

    let mut timg0 = TimerGroup::new(_peripherals.TIMG0);
    timg0.wdt.disable();
    let sw_int = SoftwareInterruptControl::new(_peripherals.SW_INTERRUPT);

    info!("Hello");

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
    let delay = Delay::new();
    let mut s3 = S3interface::new(reset, vpp, sclk, sdat);

    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
    let station_config = Config::Station(
        StationConfig::default()
            .with_ssid(SSID)
            .with_password(PASSWORD.into()),
    );
    info!("init wifi begin");
    let (mut controller, interfaces) = esp_radio::wifi::new(
        _peripherals.WIFI,
        ControllerConfig::default().with_initial_config(station_config),
    )
    .unwrap();

    info!("Wifi configured and started!");
    let wifi_interface = interfaces.station;

    let config = embassy_net::Config::dhcpv4(Default::default());

    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    // Init network stack
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        config,
        mk_static!(StackResources<3>, StackResources::<3>::new()),
        seed,
    );
    info!("delay for power");
    delay.delay_millis(800);
    info!("Scan");
    let scan_config = ScanConfig::default().with_max(1);
    let result = controller.scan_async(&scan_config).await.unwrap();
    for ap in result {
        println!("{:?}", ap);
    }

    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(net_task(runner).unwrap());

    stack.wait_config_up().await;

    if let Some(config) = stack.config_v4() {
        println!("Got IP: {}", config.address);
    }

    loop {
        delay.delay_millis(10);
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    info!("start connection task");

    loop {
        info!("About to connect...");

        match controller.connect_async().await {
            Ok(info) => {
                println!("Wifi connected to {:?}", info);

                // wait until we're no longer connected
                let info = controller.wait_for_disconnect_async().await.ok();
                println!("Disconnected: {:?}", info);
            }
            Err(e) => {
                println!("Failed to connect to wifi: {e:?}");
            }
        }

        Timer::after(Duration::from_millis(5000)).await
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await
}
