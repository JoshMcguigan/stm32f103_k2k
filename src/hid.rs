// Copyright 2019 Robin Krahl <robin.krahl@ireas.org>, Guillaume Pinot <texitoi@texitoi.eu>
// SPDX-License-Identifier: Apache-2.0 OR MIT

use keytokey::KeyCode;
use usb_device::bus::{InterfaceNumber, StringIndex, UsbBus, UsbBusAllocator};
use usb_device::class::{ControlIn, ControlOut, UsbClass};
use usb_device::control;
use usb_device::control::{Recipient, RequestType};
use usb_device::descriptor::DescriptorWriter;
use usb_device::endpoint::{EndpointAddress, EndpointIn};
use usb_device::UsbError;

const SPECIFICATION_RELEASE: u16 = 0x111;
const INTERFACE_CLASS_HID: u8 = 0x03;

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum Subclass {
    None = 0x00,
    BootInterface = 0x01,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum Protocol {
    None = 0x00,
    Keyboard = 0x01,
    Mouse = 0x02,
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum DescriptorType {
    Hid = 0x21,
    Report = 0x22,
    _Physical = 0x23,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum Request {
    GetReport = 0x01,
    GetIdle = 0x02,
    GetProtocol = 0x03,
    SetReport = 0x09,
    SetIdle = 0x0a,
    SetProtocol = 0x0b,
}
impl Request {
    fn new(u: u8) -> Option<Request> {
        use Request::*;
        match u {
            0x01 => Some(GetReport),
            0x02 => Some(GetIdle),
            0x03 => Some(GetProtocol),
            0x09 => Some(SetReport),
            0x0a => Some(SetIdle),
            0x0b => Some(SetProtocol),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReportType {
    Input,
    Output,
    Feature,
    Reserved(u8),
}

impl From<u8> for ReportType {
    fn from(val: u8) -> Self {
        match val {
            1 => ReportType::Input,
            2 => ReportType::Output,
            3 => ReportType::Feature,
            _ => ReportType::Reserved(val),
        }
    }
}

pub trait HidDevice {
    fn subclass(&self) -> Subclass;

    fn protocol(&self) -> Protocol;

    fn report_descriptor(&self) -> &[u8];

    fn set_report(&mut self, report_type: ReportType, report_id: u8, data: &[u8])
        -> Result<(), ()>;

    fn get_report(&mut self, report_type: ReportType, report_id: u8) -> Result<&[u8], ()>;
}

pub struct HidClass<'a, B: UsbBus, D: HidDevice> {
    device: D,
    interface: InterfaceNumber,
    endpoint_interrupt_in: EndpointIn<'a, B>,
    expect_interrupt_in_complete: bool,
}

impl<B: UsbBus, D: HidDevice> HidClass<'_, B, D> {
    pub fn new(device: D, alloc: &UsbBusAllocator<B>) -> HidClass<'_, B, D> {
        HidClass {
            device,
            interface: alloc.interface(),
            endpoint_interrupt_in: alloc.interrupt(8, 10),
            expect_interrupt_in_complete: false,
        }
    }

    pub fn write(&mut self, data: &[u8]) -> Result<usize, ()> {
        if self.expect_interrupt_in_complete {
            return Ok(0);
        }

        if data.len() >= 8 {
            self.expect_interrupt_in_complete = true;
        }

        match self.endpoint_interrupt_in.write(data) {
            Ok(count) => Ok(count),
            Err(UsbError::WouldBlock) => Ok(0),
            Err(_) => Err(()),
        }
    }

    fn get_report(&mut self, xfer: ControlIn<B>) {
        let req = xfer.request();
        let [report_type, report_id] = req.value.to_be_bytes();
        let report_type = ReportType::from(report_type);
        match self.device.get_report(report_type, report_id) {
            Ok(data) => xfer.accept_with(data).ok(),
            Err(()) => xfer.reject().ok(),
        };
    }

    fn set_report(&mut self, xfer: ControlOut<B>) {
        let req = xfer.request();
        let [report_type, report_id] = req.value.to_be_bytes();
        let report_type = ReportType::from(report_type);
        match self.device.set_report(report_type, report_id, xfer.data()) {
            Ok(()) => xfer.accept().ok(),
            Err(()) => xfer.reject().ok(),
        };
    }
}

impl<B: UsbBus, D: HidDevice> UsbClass<B> for HidClass<'_, B, D> {
    fn poll(&mut self) {}

    fn reset(&mut self) {
        self.expect_interrupt_in_complete = false;
    }

    fn get_configuration_descriptors(
        &self,
        writer: &mut DescriptorWriter,
    ) -> usb_device::Result<()> {
        writer.interface(
            self.interface,
            INTERFACE_CLASS_HID,
            self.device.subclass() as u8,
            self.device.protocol() as u8,
        )?;

        let report_descriptor = self.device.report_descriptor();
        let descriptor_len = report_descriptor.len();
        if descriptor_len > u16::max_value() as usize {
            return Err(UsbError::InvalidState);
        }
        let descriptor_len = (descriptor_len as u16).to_le_bytes();
        let specification_release = SPECIFICATION_RELEASE.to_le_bytes();
        writer.write(
            DescriptorType::Hid as u8,
            &[
                specification_release[0],     // bcdHID.lower
                specification_release[1],     // bcdHID.upper
                0,                            // bCountryCode: 0 = not supported
                1,                            // bNumDescriptors
                DescriptorType::Report as u8, // bDescriptorType
                descriptor_len[0],            // bDescriptorLength.lower
                descriptor_len[1],            // bDescriptorLength.upper
            ],
        )?;

        writer.endpoint(&self.endpoint_interrupt_in)?;

        Ok(())
    }

    fn get_string(&self, _index: StringIndex, _lang_id: u16) -> Option<&str> {
        None
    }

    fn endpoint_in_complete(&mut self, addr: EndpointAddress) {
        if addr == self.endpoint_interrupt_in.address() {
            self.expect_interrupt_in_complete = false;
        }
    }

    fn endpoint_out(&mut self, _addr: EndpointAddress) {}

    fn control_in(&mut self, xfer: ControlIn<B>) {
        let req = xfer.request();
        match (req.request_type, req.recipient) {
            (RequestType::Standard, Recipient::Interface) => {
                if req.request == control::Request::GET_DESCRIPTOR {
                    let (dtype, index) = req.descriptor_type_index();
                    if dtype == DescriptorType::Report as u8 && index == 0 {
                        let descriptor = self.device.report_descriptor();
                        xfer.accept_with(descriptor).ok();
                    }
                }
            }
            (RequestType::Class, Recipient::Interface) => {
                if let Some(request) = Request::new(req.request) {
                    if request == Request::GetReport {
                        self.get_report(xfer);
                    }
                }
            }
            _ => {}
        }
    }

    fn control_out(&mut self, xfer: ControlOut<B>) {
        let req = xfer.request();
        if req.request_type == RequestType::Class && req.recipient == Recipient::Interface {
            if let Some(request) = Request::new(req.request) {
                match request {
                    Request::SetReport => self.set_report(xfer),
                    _ => (),
                }
            }
        }
    }
}

#[derive(Default, Clone)]
pub struct KbHidReport([u8; 8]);

impl KbHidReport {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn pressed(&mut self, kc: KeyCode) {
        use KeyCode::*;
        match kc {
            No => (),
            ErrorRollOver | PostFail | ErrorUndefined => self.set_all(kc),
            kc if kc.is_modifier() => self.0[0] |= kc.as_modifier_bit(),
            _ => self.0[2..]
                .iter_mut()
                .find(|c| **c == 0)
                .map(|c| *c = kc as u8)
                .unwrap_or_else(|| self.set_all(ErrorRollOver)),
        }
    }
    fn set_all(&mut self, kc: KeyCode) {
        for c in &mut self.0[2..] {
            *c = kc as u8;
        }
    }

    pub fn clear(&mut self) {
        self.set_all(KeyCode::No);
    }
}
