#![no_main]
#![no_std]

mod layout;

use core::sync::atomic::{AtomicBool, Ordering};

use defmt::*;
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{select, Either};
use embassy_nrf::gpio::{Input, Output, OutputDrive, Level, Pull};
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, pac, peripherals, usb};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_usb::class::hid::{HidReaderWriter, ReportId, RequestHandler, State};
use embassy_usb::control::OutResponse;
use embassy_usb::{Builder, Config, Handler};
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};
use {defmt_rtt as _, panic_probe as _};

use keyberon::matrix::Matrix;
use keyberon::layout::Layout;
use keyberon::debounce::Debouncer;

// type CorgieMatrix = Matrix<AnyPin, AnyPin, 3, 3>;

bind_interrupts!(struct Irqs {
  USBD => usb::InterruptHandler<peripherals::USBD>;
  POWER_CLOCK => usb::vbus_detect::InterruptHandler;
});

static SUSPENDED: AtomicBool = AtomicBool::new(false);

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
  let p = embassy_nrf::init(Default::default());

  info!("Enabling ext hfosc...");
  unsafe {
    pac::Peripherals::steal().CLOCK.tasks_hfclkstart.write(|w| w.bits(1));
    while pac::Peripherals::steal().CLOCK.events_hfclkstarted.read().bits() == 0 {}
  }

  let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
  let mut config = Config::new(0xc0de, 0xcafe);
  config.manufacturer = Some("Bounce");
  config.product = Some("Corgie Board");
  config.serial_number = Some("1-420");
  config.max_power = 100;
  config.max_packet_size_0 = 64;
  config.supports_remote_wakeup = true;

  let mut config_descriptor = [0; 256];
  let mut bos_descriptor = [0; 256];
  let mut msos_descriptor = [0; 256];
  let mut control_buf = [0; 64];
  let mut request_handler = CorgieRequestHandler {};
  let mut device_handler = CorgieDeviceHandler::new();
  let mut state = State::new();

  let mut builder = Builder::new(
    driver,
    config,
    &mut config_descriptor,
    &mut bos_descriptor,
    &mut msos_descriptor,
    &mut control_buf
  );
  builder.handler(&mut device_handler);

  let config = embassy_usb::class::hid::Config {
    report_descriptor: KeyboardReport::desc(),
    request_handler: None,
    poll_ms: 60,
    max_packet_size: 64
  };

  let hid = HidReaderWriter::<_, 1, 8>::new(&mut builder, &mut state, config);
  let mut usb = builder.build();
  let remote_wakeup: Signal<CriticalSectionRawMutex, ()> = Signal::new();

  let rows = [
    Input::new(p.P0_17, Pull::Up),
    Input::new(p.P0_20, Pull::Up),
    Input::new(p.P0_22, Pull::Up)
  ];

  let columns = [
    Output::new(p.P0_24, Level::Low, OutputDrive::Standard),
    Output::new(p.P0_00, Level::Low, OutputDrive::Standard),
    Output::new(p.P0_11, Level::Low, OutputDrive::Standard)
  ];

  let mut matrix = Matrix::new(rows, columns).unwrap();
  let mut layout = Layout::new(&layout::LAYERS);
  let mut debouncer = Debouncer::new([[false; 3]; 3], [[false; 3]; 3], 5);

  let (reader, mut writer) = hid.split();

  // Run the USB device task
  let usb_future = async {
    loop {
      usb.run_until_suspend().await;
      match select(usb.wait_resume(), remote_wakeup.wait()).await {
        Either::First(_) => (),
        Either::Second(_) => unwrap!(usb.remote_wakeup().await)
      }
    }
  };

  let kb_future = async {
    loop {
      let Ok(keys) = matrix.get();
      for event in debouncer.events(keys) {
        layout.event(event);
      }

      let mut report = KeyboardReport {
        modifier: 0,
        reserved: 0,
        leds: 0,
        keycodes: [0; 6]
      };

      for (idx, key) in layout.keycodes().take(6).enumerate() {
        report.keycodes[idx] = key as u8;
      }

      if writer.write_serialize(&report).await.is_err() {
        // TODO: Handle fail write
      }
    }
  };

  let out_future = async {
    reader.run(false, &mut request_handler).await;
  };

  join(usb_future, join(kb_future, out_future)).await;
}

struct CorgieRequestHandler {}

impl RequestHandler for CorgieRequestHandler {
  fn get_report(&mut self, id: ReportId, _buf: &mut [u8]) -> Option<usize> {
    info!("Get report for {:?}", id);
    None
  }

  fn set_report(&mut self, id: ReportId, data: &[u8]) -> OutResponse {
    info!("Set report for {:?}: {=[u8]}", id, data);
    OutResponse::Accepted
  }

  fn set_idle_ms(&mut self, id: Option<ReportId>, dur: u32) {
    info!("Set idle rate for {:?} to {:?}", id, dur);
  }

  fn get_idle_ms(&mut self, id: Option<ReportId>) -> Option<u32> {
    info!("Get idle rate for {:?}", id);
    None
  }
}

struct CorgieDeviceHandler {
  configured: AtomicBool
}

impl CorgieDeviceHandler {
  fn new() -> Self {
    Self { configured: AtomicBool::new(false) }
  }
}

impl Handler for CorgieDeviceHandler {
  fn enabled(&mut self, _enabled: bool) {
    self.configured.store(false, Ordering::Relaxed);
    SUSPENDED.store(false, Ordering::Release);
  }

  fn reset(&mut self) {
    self.configured.store(false, Ordering::Relaxed);
  }

  fn addressed(&mut self, _addr: u8) {
    self.configured.store(false, Ordering::Relaxed);
  }

  fn configured(&mut self, configured: bool) {
    self.configured.store(configured, Ordering::Relaxed);
  }

  fn suspended(&mut self, suspended: bool) {
    if suspended {
      SUSPENDED.store(true, Ordering::Release);
    } else {
      SUSPENDED.store(false, Ordering::Release);
      if self.configured.load(Ordering::Relaxed) {
        // Device resumed
      }
    }
  }
}
