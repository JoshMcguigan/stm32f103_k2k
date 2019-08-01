#![no_main]
#![no_std]
#![feature(alloc_error_handler)]

extern crate panic_halt;
#[macro_use(block)]
extern crate nb;
//use no_std_compat::prelude::v1::*;

extern crate alloc;
extern crate alloc_cortex_m;
extern crate cortex_m_rt as rt; // v0.5.x

use core::alloc::Layout;

#[alloc_error_handler]
fn oom(_info: Layout, //~ ERROR argument should be `Layout`
) -> ! //~ ERROR return type should be `!`
{
    loop {}
}

use alloc_cortex_m::CortexMHeap;

#[global_allocator]
static ALLOCATOR: CortexMHeap = CortexMHeap::empty();

/*
#[allow(unused)]
macro_rules! dbg {
    ($val:expr) => {
        // Use of `match` here is intentional because it affects the lifetimes
        // of temporaries - https://stackoverflow.com/a/48732525/1063961
        match $val {
            tmp => {
                use core::fmt::Write;
                let mut out = cortex_m_semihosting::hio::hstdout().unwrap();
                writeln!(
                    out,
                    "[{}:{}] {} = {:#?}",
                    file!(),
                    line!(),
                    stringify!($val),
                    &tmp
                )
                .unwrap();
                tmp
            }
        }
    };
}
*/

pub mod hid;
pub mod keyboard;
pub mod matrix;

use crate::keyboard::Keyboard;
use crate::matrix::Matrix;
use no_std_compat::prelude::v1::*;
use rtfm::app;

//use stm32f1xx_hal::prelude::*; can't use this with v2 digital traits
use stm32_usbd::{UsbBus, UsbBusType};
//use stm32f1xx_hal::gpio::{Alternate, Floating, Input, PushPull};
//use stm32f1xx_hal::prelude::*;
pub use stm32f1xx_hal::afio::AfioExt as _stm32_hal_afio_AfioExt;
pub use stm32f1xx_hal::dma::DmaExt as _stm32_hal_dma_DmaExt;
pub use stm32f1xx_hal::flash::FlashExt as _stm32_hal_flash_FlashExt;
pub use stm32f1xx_hal::gpio::GpioExt as _stm32_hal_gpio_GpioExt;
//pub use stm32f1xx_hal::hal::adc::OneShot as _embedded_hal_adc_OneShot;
//pub use stm32f1xx_hal::hal::digital::StatefulOutputPin as _embedded_hal_digital_StatefulOutputPin;
//pub use stm32f1xx_hal::hal::digital::ToggleableOutputPin as _embedded_hal_digital_ToggleableOutputPin;
//pub use stm32f1xx_hal::hal::prelude::*;
pub use stm32f1xx_hal::dma::CircReadDma as _stm32_hal_dma_CircReadDma;
pub use stm32f1xx_hal::dma::ReadDma as _stm32_hal_dma_ReadDma;
pub use stm32f1xx_hal::dma::WriteDma as _stm32_hal_dma_WriteDma;
pub use stm32f1xx_hal::pwm::PwmExt as _stm32_hal_pwm_PwmExt;
pub use stm32f1xx_hal::rcc::RccExt as _stm32_hal_rcc_RccExt;
pub use stm32f1xx_hal::time::U32Ext as _stm32_hal_time_U32Ext;

#[allow(deprecated)]
use embedded_hal::digital::v1::ToggleableOutputPin;
use embedded_hal::digital::v2::OutputPin;
#[allow(unused_imports)]
use embedded_hal::digital::v2_compat;
use embedded_hal::serial::Write;

use stm32f1;
use stm32f1xx_hal::stm32;
use stm32f1xx_hal::{gpio, serial, timer};
use usb_device::bus;
use usb_device::class::UsbClass;
use usb_device::prelude::*;

type KeyboardHidClass = hid::HidClass<'static, UsbBusType, Keyboard>;
type Led = gpio::gpioc::PC13<gpio::Output<gpio::PushPull>>;

// Generic keyboard from
// https://github.com/obdev/v-usb/blob/master/usbdrv/USB-IDs-for-free.txt
const VID: u16 = 0x27db;
const PID: u16 = 0x16c0;

pub trait StringSender {
    fn writeln(&mut self, s: &str);
}

impl StringSender for serial::Tx<stm32f1::stm32f103::USART1> {
    fn writeln(&mut self, s: &str) {
        for b in s.bytes() {
            block!(self.write(b)).ok();
        }
        block!(self.write(b'\r')).ok();
        block!(self.write(b'\n')).ok();
    }
}

#[app(device = stm32f1xx_hal::stm32)]
const APP: () = {
    static mut USB_DEV: UsbDevice<'static, UsbBusType> = ();
    static mut USB_CLASS: KeyboardHidClass = ();
    static mut TIMER: timer::Timer<stm32::TIM3> = ();
    static mut TX: serial::Tx<stm32f1::stm32f103::USART1> = ();
    static mut RX: serial::Rx<stm32f1::stm32f103::USART1> = ();
    static mut LED: Led = ();
    static mut MATRIX: Matrix = ();

    #[init]
    fn init() -> init::LateResources {
        let start = rt::heap_start() as usize;
        let size = 4096; // in bytes
        unsafe { ALLOCATOR.init(start, size) }

        static mut USB_BUS: Option<bus::UsbBusAllocator<UsbBusType>> = None;

        let mut flash = device.FLASH.constrain();
        let mut rcc = device.RCC.constrain();

        let clocks = rcc
            .cfgr
            .use_hse(8.mhz())
            .sysclk(48.mhz())
            .pclk1(24.mhz())
            .freeze(&mut flash.acr);

        let mut gpioa = device.GPIOA.split(&mut rcc.apb2);
        let mut gpiob = device.GPIOB.split(&mut rcc.apb2);
        let mut gpioc = device.GPIOC.split(&mut rcc.apb2);

        let mut led = gpioc.pc13.into_push_pull_output(&mut gpioc.crh);
        led.set_high().ok();

        // BluePill board has a pull-up resistor on the D+ line.
        // Pull the D+ pin down to send a RESET condition to the USB bus.
        let mut usb_dp = gpioa.pa12.into_push_pull_output(&mut gpioa.crh);
        usb_dp.set_low().ok();
        cortex_m::asm::delay(clocks.sysclk().0 / 100);

        let usb_dm = gpioa.pa11;
        let usb_dp = usb_dp.into_floating_input(&mut gpioa.crh);

        unsafe {
            USB_BUS = Some(UsbBus::new(device.USB, (usb_dm, usb_dp)));
        }
        let usb_bus = unsafe { USB_BUS.as_ref().unwrap() };

        let usb_class = hid::HidClass::new(Keyboard::new(), &usb_bus);
        let usb_dev = UsbDeviceBuilder::new(usb_bus, UsbVidPid(VID, PID))
            .manufacturer("TyberiusPrime")
            .product("K2KAdvantage")
            .serial_number(env!("CARGO_PKG_VERSION"))
            .build();

        let mut timer = timer::Timer::tim3(device.TIM3, 3.hz(), clocks, &mut rcc.apb1); //todo, do this faster ;)
        timer.listen(timer::Event::Update);

        let pin_tx = gpioa.pa9.into_alternate_push_pull(&mut gpioa.crh);
        let pin_rx = gpioa.pa10;
        let mut afio = device.AFIO.constrain(&mut rcc.apb2);

        let ser = serial::Serial::usart1(
            device.USART1,
            (pin_tx, pin_rx),
            &mut afio.mapr,
            9_600.bps(),
            clocks,
            &mut rcc.apb2,
        );
        let (mut tx, rx) = ser.split();
        tx.writeln("Up");
        let matrix = Matrix::new(
            vec![
                gpioa.pa7.into_pull_up_input(&mut gpioa.crl).downgrade(),
                gpioa.pa6.into_pull_up_input(&mut gpioa.crl).downgrade(),
                gpioa.pa5.into_pull_up_input(&mut gpioa.crl).downgrade(),
                gpioa.pa4.into_pull_up_input(&mut gpioa.crl).downgrade(),
                gpioa.pa3.into_pull_up_input(&mut gpioa.crl).downgrade(),
                gpioa.pa2.into_pull_up_input(&mut gpioa.crl).downgrade(),
            ],
            vec![
                gpiob.pb11.into_pull_up_input(&mut gpiob.crh).downgrade(),
                gpiob.pb10.into_pull_up_input(&mut gpiob.crh).downgrade(),
                gpiob.pb1.into_pull_up_input(&mut gpiob.crl).downgrade(),
                gpiob.pb0.into_pull_up_input(&mut gpiob.crl).downgrade(),
            ],
            vec![
                gpioa.pa8.into_push_pull_output(&mut gpioa.crh).downgrade(),
                gpioa.pa15.into_push_pull_output(&mut gpioa.crh).downgrade(),
            ],
            vec![
                gpiob.pb12.into_push_pull_output(&mut gpiob.crh).downgrade(),
                gpiob.pb13.into_push_pull_output(&mut gpiob.crh).downgrade(),
                gpiob.pb14.into_push_pull_output(&mut gpiob.crh).downgrade(),
                gpiob.pb15.into_push_pull_output(&mut gpiob.crh).downgrade(),
                gpiob.pb3.into_push_pull_output(&mut gpiob.crl).downgrade(),
                gpiob.pb4.into_push_pull_output(&mut gpiob.crl).downgrade(),
                gpiob.pb5.into_push_pull_output(&mut gpiob.crl).downgrade(),
            ],
        );

        init::LateResources {
            USB_DEV: usb_dev,
            USB_CLASS: usb_class,
            TIMER: timer,
            TX: tx,
            RX: rx,
            LED: led,
            MATRIX: matrix,
            /*
            MATRIX: matrix::Matrix::new(
                matrix::Cols(
                    gpiob.pb12.into_pull_up_input(&mut gpiob.crh),
                    gpiob.pb13.into_pull_up_input(&mut gpiob.crh),
                    gpiob.pb14.into_pull_up_input(&mut gpiob.crh),
                    gpiob.pb15.into_pull_up_input(&mut gpiob.crh),
                    gpioa.pa8.into_pull_up_input(&mut gpioa.crh),
                    gpioa.pa9.into_pull_up_input(&mut gpioa.crh),
                    gpioa.pa10.into_pull_up_input(&mut gpioa.crh),
                    gpiob.pb5.into_pull_up_input(&mut gpiob.crl),
                    gpiob.pb6.into_pull_up_input(&mut gpiob.crl),
                    gpiob.pb7.into_pull_up_input(&mut gpiob.crl),
                    gpiob.pb8.into_pull_up_input(&mut gpiob.crh),
                    gpiob.pb9.into_pull_up_input(&mut gpiob.crh),
                ),
                matrix::Rows(
                    gpiob.pb11.into_push_pull_output(&mut gpiob.crh),
                    gpiob.pb10.into_push_pull_output(&mut gpiob.crh),
                    gpiob.pb1.into_push_pull_output(&mut gpiob.crl),
                    gpiob.pb0.into_push_pull_output(&mut gpiob.crl),
                    gpioa.pa7.into_push_pull_output(&mut gpioa.crl),
                ),
            ),
                */
        }
    }

    #[interrupt(priority = 2, resources = [USB_DEV, USB_CLASS])]
    fn USB_HP_CAN_TX() {
        usb_poll(&mut resources.USB_DEV, &mut resources.USB_CLASS);
    }

    #[interrupt(priority = 2, resources = [USB_DEV, USB_CLASS])]
    fn USB_LP_CAN_RX0() {
        usb_poll(&mut resources.USB_DEV, &mut resources.USB_CLASS);
    }

    #[interrupt(priority = 1, resources = [USB_CLASS, //MATRIX, 
    TIMER, TX, LED, MATRIX])]
    fn TIM3() {
        resources.TIMER.clear_update_interrupt_flag();
        resources.TX.writeln("Hi!");
        #[allow(deprecated)]
        resources.LED.toggle();
        resources.MATRIX.read_matrix();
        resources.MATRIX.debug_serial(resources.TX);

        /*
        if resources.DEBOUNCER.update(resources.MATRIX.get()) {
            let data = resources.DEBOUNCER.get();
            let mut report = key_code::KbHidReport::default();
            for kc in resources.LAYOUT.key_codes(data.iter_pressed()) {
                report.pressed(kc);
            }
            while let Ok(0) = resources.USB_CLASS.lock(|k| k.write(report.as_bytes())) {}
        }
        */
    }
};

fn usb_poll(usb_dev: &mut UsbDevice<'static, UsbBusType>, keyboard: &mut KeyboardHidClass) {
    if usb_dev.poll(&mut [keyboard]) {
        keyboard.poll();
    }
}
