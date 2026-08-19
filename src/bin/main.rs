#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]
#![recursion_limit = "512"]
extern crate alloc;
use alloc::format;
use critical_section;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use embedded_hal::delay;
use esp_backtrace as _;

use esp_alloc as _;
//use core::fmt::DebugList;
//use embedded_hal::delay::DelayNs;
use defmt::info;
use embassy_net::{
    Runner, Stack, StackResources,
    dns::DnsSocket,
    tcp::TcpSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use esp_hal::delay::Delay;
use esp_hal::gpio::{Flex, Level, Output, OutputConfig};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::ram;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{clock::CpuClock, riscv::register::hpmcounter15h::read};

use esp_println as _;
use esp_println::println;
use esp_radio::wifi::{
    Config, ControllerConfig, Interface, WifiController, scan::ScanConfig, sta::StationConfig,
};

use picoserve::request::RequestBodyReader;
use picoserve::response::IntoResponse;
use picoserve::routing::post_service;
use picoserve::routing::{get, post};
use picoserve::{AppBuilder, io, request::Request, routing::get_service};
use picoserve::{AppRouter, io::Read};

#[panic_handler]
fn panic(panic: &core::panic::PanicInfo) -> ! {
    println!("Panic {}", panic);
    loop {}
}
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

const MEM_SIZE: usize = 255usize;

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
        self.start_condition();
        self.send_byte(byte1);
        self.send_byte((addr >> 8) as u8);
        self.send_byte(addr as u8);
        let mut mem: [u8; MEM_SIZE] = [0; MEM_SIZE];
        for i in 0..len {
            mem[i] = self.read_byte();
        }
        self.stop_condition();
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

#[embassy_executor::task]
async fn wifi_task() {}

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
    static s3_static: static_cell::StaticCell<S3interface> = static_cell::StaticCell::new();
    let s3: &'static mut S3interface = s3_static.init(s3);

    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
    spawner.spawn(s3_interface_task(s3).unwrap());

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

    println!("Now try connect wifi");
    println!("ID : {}", SSID);
    println!("PW : {}", PASSWORD);

    loop {
        if let Some(config) = stack.config_v4() {
            println!("Got IP: {}", config.address);
            break;
        }
        Timer::after_secs(5).await;
        println!("Wait for secs ... ");
    }

    static STACK: static_cell::StaticCell<embassy_net::Stack> = static_cell::StaticCell::new();
    let stack: &'static embassy_net::Stack = STACK.init(stack);
    spawner.spawn(web_task(&stack).unwrap());

    loop {
        Timer::after_secs(10).await;
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

#[derive(Copy, Clone, PartialEq)]
enum RecordType {
    Data,
    Eof,
    ExtendedSegmentAddress,
    StartSegmentAddress,
    ExtendedLinearAddress,
    StartLinearAddress,
}

#[derive(Copy, Clone)]
pub struct HexRecord {
    pub record_type: RecordType,
    pub byte_count: u8,
    pub address: u16,
    pub data: [u8; 255],
}
impl HexRecord {
    pub const fn new() -> HexRecord {
        HexRecord {
            record_type: (RecordType::Eof),
            byte_count: (0u8),
            address: (0u16),
            data: ([0u8; 255]),
        }
    }
}

const RECORDS: usize = 100usize;
struct HexFile {
    pub records: [HexRecord; RECORDS],
    pub record_count: usize,
}

impl HexFile {
    pub const fn new() -> HexFile {
        HexFile {
            records: ([HexRecord::new(); RECORDS]),
            record_count: 0usize,
        }
    }
}
static HEX_FILE: Mutex<CriticalSectionRawMutex, HexFile> = Mutex::new(HexFile::new());

enum WebCommand {
    Program,
    Erase,
    Verify,
    Auto,
}

static WEB_COMMAND_SIGNAL: Signal<CriticalSectionRawMutex, WebCommand> = Signal::new();
enum HexStage {
    StartCode,
    ByteCount,
    Address,
    RecordType,
    Data,
    Checksum,
}
fn u8char2num16(ch: u8) -> Option<u16> {
    Some(u8char2num(ch).unwrap() as u16)
}
fn u8char2num(ch: u8) -> Option<u8> {
    if ch >= '0' as u8 && ch <= '9' as u8 {
        return Some(ch - ('0' as u8));
    }
    if ch >= 'a' as u8 && ch <= 'f' as u8 {
        return Some(ch - ('a' as u8) + 0xa);
    }
    if ch >= 'A' as u8 && ch <= 'F' as u8 {
        return Some(ch - ('A' as u8) + 0xa);
    }
    None
}
fn str2u8(array: [u8; 2]) -> Option<u8> {
    Some((u8char2num(array[0]).unwrap() << 4) + (u8char2num(array[1]).unwrap()))
}
fn str2u16(array: [u8; 4]) -> Option<u16> {
    Some(
        (u8char2num16(array[0]).unwrap() << 12)
            + (u8char2num16(array[1]).unwrap() << 8)
            + (u8char2num16(array[2]).unwrap() << 4)
            + u8char2num16(array[3]).unwrap(),
    )
}
struct UploadProc;

impl picoserve::routing::RequestHandlerService<()> for UploadProc {
    async fn call_request_handler_service<
        R: Read,
        W: picoserve::response::ResponseWriter<Error = R::Error>,
    >(
        &self,
        (): &(),
        (): (),
        mut request: picoserve::request::Request<'_, R>,
        response_writer: W,
    ) -> Result<picoserve::ResponseSent, W::Error> {
        if request.body_connection.content_length() > 2_000_000 {
            let response = (
                picoserve::response::StatusCode::PAYLOAD_TOO_LARGE,
                "The file must be smaller than 2MB",
            )
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
            return response;
        }

        let timeout = embassy_time::Duration::from_micros(
            request.body_connection.content_length() as u64 * 10,
        );
        let start_time = embassy_time::Instant::now();

        let mut reader = request
            .body_connection
            .body()
            .reader()
            .with_different_timeout(timeout);

        let mut read_buffer = [0; 512];
        let mut hex_file = HEX_FILE.lock().await;
        let mut seek: usize;
        let mut current_record = 0usize;
        let mut stage: HexStage = HexStage::StartCode;
        let mut readed_byte_count: usize;
        let mut field_buffer: [u8; 4] = [0u8; 4];
        let mut field_buffer_seek = 0usize;
        let mut record_index: usize = 0usize;
        let mut data_seek: usize = 0usize;
        let mut read_done_flag: bool = false;

        info!("Start read");
        loop {
            let read_size = reader.read(&mut read_buffer).await?;
            seek = 0usize;

            if read_done_flag {
                break;
            }
            if read_size == 0 {
                break;
            }
            let read_buf = &read_buffer[..read_size];
            let read_str = core::str::from_utf8(read_buf).unwrap();

            println!("read size {} ", read_size);

            for c in read_str.chars() {
                // println!("read char {} ", c);
                let c = c as u8;
                // println!("read char {} ",c);
                if read_done_flag {
                    break;
                }
                field_buffer[field_buffer_seek] = c;
                field_buffer_seek = field_buffer_seek + 1;
                match stage {
                    HexStage::StartCode => {
                        field_buffer_seek = 0usize;
                        if field_buffer[0] == ':' as u8 {
                            // info!("find start code");
                            stage = HexStage::ByteCount;
                        }
                    }
                    HexStage::ByteCount => {
                        if field_buffer_seek == 2 {
                            let [f, s, _, _] = field_buffer;
                            hex_file.records[record_index].byte_count = str2u8([f, s]).unwrap();
                            // println!(
                            //     "record byte count {}",
                            //     hex_file.records[record_index].byte_count
                            // );
                            stage = HexStage::Address;
                            field_buffer_seek = 0usize;
                        }
                    }
                    HexStage::Address => {
                        if field_buffer_seek == 4 {
                            stage = HexStage::RecordType;
                            hex_file.records[record_index].address = str2u16(field_buffer).unwrap();
                            field_buffer_seek = 0usize;
                        }
                    }
                    HexStage::RecordType => {
                        if field_buffer_seek == 2 {
                            stage = HexStage::Data;
                            let [f, s, _, _] = field_buffer;
                            let record_type = str2u8([f, s]).unwrap();
                            // println!("Record type {}", record_type);
                            let record_type = match record_type {
                                0u8 => RecordType::Data,
                                1u8 => RecordType::Eof,
                                2u8 => RecordType::ExtendedSegmentAddress,
                                3u8 => RecordType::StartSegmentAddress,
                                4u8 => RecordType::ExtendedLinearAddress,
                                5u8 => RecordType::StartLinearAddress,
                                _ => {
                                    panic!("Hex file Error")
                                }
                            };

                            hex_file.records[record_index].record_type = record_type;
                            data_seek = 0usize;
                            field_buffer_seek = 0usize;
                        }
                    }
                    HexStage::Data => {
                        if field_buffer_seek == 2 {
                            field_buffer_seek = 0usize;
                            let [f, s, _, _] = field_buffer;
                            let data = str2u8([f, s]).unwrap();
                            hex_file.records[record_index].data[data_seek] = data;
                            data_seek = data_seek + 1;
                            if data_seek >= hex_file.records[record_index].byte_count as usize {
                                stage = HexStage::Checksum;
                            }
                        }
                    }
                    HexStage::Checksum => {
                        stage = HexStage::StartCode;

                        if hex_file.records[record_index].record_type == RecordType::Eof {
                            info!("find EOF");
                            read_done_flag = true;
                            hex_file.record_count = record_index + 1;
                        } // plz forget about checksum
                        record_index = record_index + 1;
                    }
                }
                seek = seek + 1;
            }
        }

        let result = format!("Hex file records :  {} ", hex_file.record_count);
        println!("{}", result);
        let result = result.as_bytes();
        let buffer: &[u8] = &read_buffer;
        let response = (
            picoserve::response::StatusCode::OK,
            result, // 혹은 실제 해시 결과 문자열/바이트
        );

        // 2. body_connection을 finalize()한 연결 객체와 response_writer를 함께 전달
        response
            .write_to(request.body_connection.finalize().await?, response_writer)
            .await
    }
}

#[embassy_executor::task(pool_size = 2)]
async fn web_task(stack: &'static Stack<'static>) {
    // 1. Task 내부에서 직접 router 생성 (impl 반환값이나 type alias 필요 없음)

    let html = picoserve::response::File::html(include_str!("index.html"));
    let router = picoserve::Router::new()
        .route("/", get_service(html))
        .route("/health", get(|| async move { "OK" }))
        .route("/upload", post_service(UploadProc))
        .route(
            "/program",
            post(|| async move {
                WEB_COMMAND_SIGNAL.signal(WebCommand::Program);
                info!("recived program command ");
                "now programing"
            }),
        )
        .route(
            "/erase",
            post(|| async move {
                WEB_COMMAND_SIGNAL.signal(WebCommand::Erase);
                "erase done "
            }),
        )
        .route(
            "/verify",
            post(|| async move {
                WEB_COMMAND_SIGNAL.signal(WebCommand::Verify);
                "verify done "
            }),
        )
        .route(
            "/auto",
            post(|| async move {
                WEB_COMMAND_SIGNAL.signal(WebCommand::Auto);
                "auto done"
            }),
        );

    let config = picoserve::Config::new(picoserve::Timeouts {
        start_read_request: Duration::from_secs(100),
        read_request: Duration::from_secs(100),
        persistent_start_read_request: Duration::from_secs(100),
        write: Duration::from_secs(100),
    });

    let mut rx_buffer = [0u8; 1024];
    let mut tx_buffer = [0u8; 1024];
    let mut http_buffer = [0u8; 1024];

    loop {
        let mut socket = TcpSocket::new(*stack, &mut rx_buffer, &mut tx_buffer);

        if let Err(e) = socket.accept(80).await {
            info!("Socket accept error: {:?}", e);
            continue;
        }
        info!("new socket");
        let _ = picoserve::Server::new(&router, &config, &mut http_buffer)
            .serve(socket)
            .await;
    }
}

#[embassy_executor::task]
async fn s3_interface_task(s3: &'static mut S3interface<'static>) {
    loop {
        info!("s3 interface task run");
        let web_command = WEB_COMMAND_SIGNAL.wait().await;
        info!("get signal");
        match web_command {
            WebCommand::Erase => critical_section::with(|_cs| {
                s3.init();
                s3.enter_program_mode();
                s3.erase();
            }),
            WebCommand::Program => {
                let hex_file = HEX_FILE.lock().await;
                let mut address = 0u16;
                let mut record_index = 0usize;
                critical_section::with(|_cs| {
                    loop {
                        match hex_file.records[record_index].record_type {
                            RecordType::Data => {
                                let addr: usize =
                                    (hex_file.records[record_index].address + address) as usize;
                                let len = hex_file.records[record_index].byte_count as usize;
                                s3.init();
                                s3.enter_program_mode();
                                s3.write_all(addr, hex_file.records[record_index].data, len);
                                s3.delay.delay_millis(10);
                            }
                            RecordType::Eof => {
                                // println!("Program done");
                                break;
                            }
                            RecordType::ExtendedSegmentAddress => {
                                let [a, b, c, d, ..] = hex_file.records[record_index].data;
                                let a = (a as u32) << 24;
                                let b = (b as u32) << 16;
                                let c = (c as u32) << 8;
                                let d = d as u32;

                                address = (a + b + c + d) as u16;
                                address = address << 4;
                                // println!("address changed at {}", address);
                            }

                            RecordType::StartLinearAddress => {}
                            RecordType::ExtendedLinearAddress => {}
                            RecordType::StartSegmentAddress => {}
                        }

                        record_index += 1;
                    }
                    s3.init();
                    s3.enter_program_mode();
                    s3.write_smart_option(0x0E39, 0xA7);
                    s3.delay.delay_millis(10); // 쓰기 완료 대기
                });
                println!("program done ");
            }
            WebCommand::Verify => {
                let hex_file = HEX_FILE.lock().await;
                let mut address = 0u16;
                let mut record_index = 0usize;
                let mut verify_false = false;
                loop {
                    match hex_file.records[record_index].record_type {
                        RecordType::Data => {
                            let addr: usize =
                                (hex_file.records[record_index].address + address) as usize;
                            let len = hex_file.records[record_index].byte_count as usize;
                            let mut readed: [u8; MEM_SIZE] = [0u8; MEM_SIZE];

                            critical_section::with(|_cs| {
                                s3.init();
                                s3.enter_program_mode();
                                readed = s3.read(addr as u16, len);
                            });
                            for idx in 0..len {
                                if readed[idx] != hex_file.records[record_index].data[idx] {
                                    println!(
                                        "verify false record {record_index} addr {addr} idx {} ,  {}, require {} ",
                                        idx, readed[idx], hex_file.records[record_index].data[idx]
                                    );
                                    verify_false = true;
                                } else {
                                    // println!("verify ok {record_index} idx {idx}  {}", readed[idx]);
                                }
                            }
                        }
                        RecordType::Eof => {
                            s3.init();
                            s3.enter_program_mode();
                            let smart = s3.read_smart_option(0x0E39);
                            println!("smart option {smart}  {} ", smart == 0xA7);

                            println!("verify_false{}  ", verify_false);
                            println!("verify done");
                            break;
                        }
                        RecordType::ExtendedSegmentAddress => {
                            let [a, b, c, d, ..] = hex_file.records[record_index].data;
                            let a = (a as u32) << 24;
                            let b = (b as u32) << 16;
                            let c = (c as u32) << 8;
                            let d = d as u32;

                            address = (a + b + c + d) as u16;
                            address = address << 4;
                            println!("address changed at {}", address);
                        }

                        RecordType::StartLinearAddress => {}
                        RecordType::ExtendedLinearAddress => {}
                        RecordType::StartSegmentAddress => {}
                    }

                    record_index += 1;
                }
            }
            WebCommand::Auto => {
                println!("TEST");
                critical_section::with(|_cs| {
                    s3.init();
                    s3.enter_program_mode();
                    s3.erase();

                    // --- S3 칩 단독 테스트 시작 ---
                    println!("S3 Test Start");

                    // 1. 하드웨어 통신 생사 확인 (Smart Option 쓰기 후 읽기)
                    // [쓰기]
                    s3.init();
                    s3.enter_program_mode();
                    s3.write_smart_option(0x0E39, 0xA7);
                    s3.delay.delay_millis(10); // 쓰기 완료 대기

                    // [읽기]
                    s3.init();
                    s3.enter_program_mode();
                    let smart = s3.read_smart_option(0x0E39);

                    // 2. Erase (지우기) - 인터럽트 차단 없이 자연스럽게 2초 대기
                    s3.init();
                    s3.enter_program_mode();
                    s3.erase();

                    // 3. Write
                    let bytes = [0xau8; MEM_SIZE];
                    s3.write_all(0, bytes, MEM_SIZE);

                    // for addr in 0..5 {
                    //     s3.init();
                    //     s3.enter_program_mode();
                    //     s3.write(addr as u16, 0xAA);
                    //     // 필요시 짧은 대기 유지
                    //     s3.delay.delay_millis(10);
                    // }

                    // 4. Read
                    s3.init();
                    s3.enter_program_mode();
                    s3.start_condition();
                    let mem = s3.read(0, 10);
                    s3.stop_condition();

                    // 5. 결과 출력
                    println!("Smart Option Test: {:#04X}", smart);
                    for idx in 0..10 {
                        println!("addr {} : {}", idx, mem[idx]);
                    }
                    println!("S3 Test End");
                })
            }
        }
        Timer::after(Duration::from_millis(500)).await
    }
}
