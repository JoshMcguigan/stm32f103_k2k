use crate::KeyboardHidClass;
use keytokey::{KeyCode, KeyboardState, USBKeyOut};
use crate::hid::KbHidReport;
use core::clone::Clone;
use cortex_m;

use stm32f1;
use stm32f1xx_hal::{serial, timer};

pub struct USBOut {
    state: KeyboardState,
    pub usb_class: KeyboardHidClass,
    current_report: KbHidReport,
    pub tx: serial::Tx<stm32f1::stm32f103::USART1>,
}

impl USBOut {
    pub fn new(usb_class: KeyboardHidClass, 
    tx: serial::Tx<stm32f1::stm32f103::USART1>) -> USBOut {
        USBOut {
            state: KeyboardState::new(),
            usb_class,
            current_report: KbHidReport::default(),
            tx,
        }
    }

    fn send_report(&mut self, report: &KbHidReport) {
        while let Ok(0) = self.usb_class.write(report.as_bytes())  {}
        //cortex_m::asm::delay(4800);


    }
}

impl USBKeyOut for USBOut {
    /// send these USB Keycodes concurrently rigth away.
    fn send_keys(&mut self, keys: &[KeyCode]) {
        let mut report = KbHidReport::default();
        for k in keys {
            report.pressed(*k);
        }
        self.send_report(&report);
    }
    /// register these USB keycodes to be send on .send_registered
    fn register_key(&mut self, key: KeyCode) {
        self.current_report.pressed(key);

    }
    /// send registered keycodes (or an empty nothing-pressed status)
    fn send_registered(&mut self) {
        let report = self.current_report.clone();
        self.send_report(&report);
        self.current_report.clear();
    }

    /// helper that sends an empty status
    fn send_empty(&mut self) {
        self.send_report(&KbHidReport::default());
    }

    /// retrieve a mutable KeyboardState
    fn state(&mut self) -> &mut KeyboardState {
        return &mut self.state;
    }
}
