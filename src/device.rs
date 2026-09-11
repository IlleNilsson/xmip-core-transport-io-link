//! An IO-Link device at the far end of a line: what a test or the loopback
//! puts there so a master can be driven without a sensor in the room.
//!
//! Not a device profile. One device holds a direct parameter page, process
//! data each way, and parameters as byte vectors keyed by index and
//! subindex; it answers every M-sequence the master sends — a page byte, a
//! process data cycle, and the ISDU channel's flow control, assembling a
//! request two bytes at a time and handing the response back the same way,
//! `BUSY` until it has one.

use std::collections::HashMap;

use crate::isdu::{self, Isdu, Service};
use crate::m_sequence::{Channel, DeviceMessage, MasterMessage};

/// The error a negative response carries for an index that is not there.
pub const INDEX_NOT_AVAILABLE: [u8; 2] = [0x80, 0x11];

/// One device.
pub struct Device {
    page: [u8; 32],
    parameters: HashMap<(u16, u8), Vec<u8>>,
    process_in: Vec<u8>,
    process_out: Vec<u8>,
    request: Vec<u8>,
    response: Option<Vec<u8>>,
    response_at: usize,
}

impl Device {
    /// A device with `process_in` bytes of process data in, none of it
    /// yet, and an empty direct parameter page.
    #[must_use]
    pub fn new(process_in: usize) -> Self {
        Self {
            page: [0; 32],
            parameters: HashMap::new(),
            process_in: vec![0; process_in],
            process_out: Vec::new(),
            request: Vec::new(),
            response: None,
            response_at: 0,
        }
    }

    /// Hold `bytes` as parameter `index:subindex`.
    #[must_use]
    pub fn with_parameter(mut self, index: u16, subindex: u8, bytes: impl Into<Vec<u8>>) -> Self {
        self.parameters.insert((index, subindex), bytes.into());
        self
    }

    /// Hold `bytes` from `address` on the direct parameter page.
    #[must_use]
    pub fn with_page(mut self, address: u8, bytes: &[u8]) -> Self {
        for (slot, byte) in self.page[usize::from(address & 0x1f)..]
            .iter_mut()
            .zip(bytes)
        {
            *slot = *byte;
        }
        self
    }

    /// The bytes held as `index:subindex`, as they are now.
    #[must_use]
    pub fn parameter(&self, index: u16, subindex: u8) -> Option<&[u8]> {
        self.parameters.get(&(index, subindex)).map(Vec::as_slice)
    }

    /// The process data the master last sent.
    #[must_use]
    pub fn process_out(&self) -> &[u8] {
        &self.process_out
    }

    /// What the device presents as process data in from now on.
    pub fn set_process_in(&mut self, bytes: &[u8]) {
        self.process_in = bytes.to_vec();
    }

    /// The device's answer to one master message.
    pub fn answer(&mut self, master: &MasterMessage) -> DeviceMessage {
        if !master.process_out.is_empty() {
            self.process_out.clone_from(&master.process_out);
        }
        let want = if master.read {
            master.kind.on_request()
        } else {
            0
        };
        let on_request = match master.channel {
            Channel::Page => self.page_channel(master, want),
            Channel::Isdu => self.isdu_channel(master, want),
            Channel::Process | Channel::Diagnosis => vec![0; want],
        };
        DeviceMessage {
            process_in: if master.process_out.is_empty() {
                Vec::new()
            } else {
                self.process_in.clone()
            },
            on_request,
            event: false,
            process_invalid: self.process_in.is_empty(),
        }
    }

    fn page_channel(&mut self, master: &MasterMessage, want: usize) -> Vec<u8> {
        let at = usize::from(master.address);
        if master.read {
            return self
                .page
                .iter()
                .cycle()
                .skip(at)
                .take(want)
                .copied()
                .collect();
        }
        for (slot, byte) in self.page[at..].iter_mut().zip(&master.on_request) {
            *slot = *byte;
        }
        Vec::new()
    }

    fn isdu_channel(&mut self, master: &MasterMessage, want: usize) -> Vec<u8> {
        if master.read {
            return self.isdu_read(master.address, want);
        }
        match master.address {
            isdu::START => {
                self.request.clear();
                self.response = None;
                self.request.extend_from_slice(&master.on_request);
            }
            isdu::ABORT | isdu::IDLE => {
                self.request.clear();
                self.response = None;
                return Vec::new();
            }
            _ => self.request.extend_from_slice(&master.on_request),
        }
        if let Some(length) = Isdu::length_of(&self.request)
            && self.request.len() >= length
        {
            self.request.truncate(length);
            let response = self.serve(Isdu::decode(&self.request));
            self.response = Some(response.encode());
            self.response_at = 0;
            self.request.clear();
        }
        Vec::new()
    }

    fn isdu_read(&mut self, address: u8, want: usize) -> Vec<u8> {
        let Some(response) = self.response.as_ref() else {
            let mut busy = vec![isdu::BUSY];
            busy.resize(want, 0);
            return busy;
        };
        if address == isdu::START {
            self.response_at = 0;
        }
        let at = self.response_at;
        self.response_at += want;
        let mut chunk: Vec<u8> = response.iter().skip(at).take(want).copied().collect();
        chunk.resize(want, 0);
        if self.response_at >= response.len() {
            self.response = None;
        }
        chunk
    }

    fn serve(&mut self, request: Result<Isdu, transport::TransportError>) -> Isdu {
        let negative = |service| {
            Isdu::response(service, &INDEX_NOT_AVAILABLE).unwrap_or_else(|_| Isdu::read(0, 0))
        };
        let Ok(request) = request else {
            return negative(Service::WriteResponseNegative);
        };
        let key = (request.index, request.subindex);
        match request.service {
            Service::WriteRequest if self.parameters.contains_key(&key) => {
                self.parameters.insert(key, request.data);
                Isdu::response(Service::WriteResponsePositive, &[])
                    .unwrap_or_else(|_| negative(Service::WriteResponseNegative))
            }
            Service::ReadRequest => match self.parameters.get(&key) {
                Some(bytes) => Isdu::response(Service::ReadResponsePositive, bytes)
                    .unwrap_or_else(|_| negative(Service::ReadResponseNegative)),
                None => negative(Service::ReadResponseNegative),
            },
            _ => negative(Service::WriteResponseNegative),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::m_sequence::Kind;

    #[test]
    fn the_page_is_read_a_byte_at_a_time_and_written_the_same() {
        let mut device = Device::new(0).with_page(0x07, &[0x01, 0x23]);
        let answer = device.answer(&MasterMessage::read(Kind::Type0, Channel::Page, 0x08));
        assert_eq!(answer.on_request, [0x23]);
        assert!(answer.process_invalid, "no process data in");
        let write =
            MasterMessage::write(Kind::Type1, Channel::Page, 0x10, &[0xaa, 0xbb]).expect("w");
        assert!(device.answer(&write).on_request.is_empty());
        let back = device.answer(&MasterMessage::read(Kind::Type1, Channel::Page, 0x10));
        assert_eq!(back.on_request, [0xaa, 0xbb]);
    }

    #[test]
    fn a_process_cycle_swaps_data_both_ways() {
        let mut device = Device::new(2);
        device.set_process_in(&[7, 8]);
        let cycle = MasterMessage::read(Kind::Type2, Channel::Process, 0).with_process(&[1, 2, 3]);
        let answer = device.answer(&cycle);
        assert_eq!(answer.process_in, [7, 8]);
        assert_eq!(answer.on_request, [0]);
        assert!(!answer.process_invalid);
        assert_eq!(device.process_out(), [1, 2, 3]);
    }

    #[test]
    fn an_isdu_is_assembled_from_the_channel_and_answered_back_through_it() {
        let mut device = Device::new(0).with_parameter(0x0100, 0, Vec::new());
        assert_eq!(
            device
                .answer(&MasterMessage::read(
                    Kind::Type1,
                    Channel::Isdu,
                    isdu::START
                ))
                .on_request,
            [isdu::BUSY, 0],
            "nothing asked yet"
        );
        let request = Isdu::write(0x0100, 0, &[1, 2, 3]).expect("write").encode();
        let mut address = isdu::START;
        for pair in request.chunks(2) {
            let mut two = pair.to_vec();
            two.resize(2, 0);
            let master =
                MasterMessage::write(Kind::Type1, Channel::Isdu, address, &two).expect("m");
            device.answer(&master);
            address = (address + 1) & isdu::COUNT_MASK;
        }
        assert_eq!(device.parameter(0x0100, 0), Some(&[1, 2, 3][..]));
        let first = device.answer(&MasterMessage::read(
            Kind::Type1,
            Channel::Isdu,
            isdu::START,
        ));
        assert_eq!(
            first.on_request,
            [0x51, 3],
            "a positive write response, three bytes"
        );
        let rest = device.answer(&MasterMessage::read(Kind::Type1, Channel::Isdu, 0));
        assert_eq!(rest.on_request[0], 0x51 ^ 3);
        let missing = Isdu::write(0x0200, 0, &[]).expect("write").encode();
        for (n, pair) in missing.chunks(2).enumerate() {
            let mut two = pair.to_vec();
            two.resize(2, 0);
            let address = if n == 0 {
                isdu::START
            } else {
                u8::try_from(n - 1).unwrap_or(0)
            };
            device.answer(
                &MasterMessage::write(Kind::Type1, Channel::Isdu, address, &two).expect("m"),
            );
        }
        let answer = device.answer(&MasterMessage::read(
            Kind::Type1,
            Channel::Isdu,
            isdu::START,
        ));
        assert_eq!(
            answer.on_request[0] >> 4,
            Service::WriteResponseNegative as u8
        );
        device.answer(
            &MasterMessage::write(Kind::Type1, Channel::Isdu, isdu::ABORT, &[0, 0]).expect("a"),
        );
        assert_eq!(
            device
                .answer(&MasterMessage::read(
                    Kind::Type1,
                    Channel::Isdu,
                    isdu::START
                ))
                .on_request[0],
            isdu::BUSY,
            "aborted"
        );
    }
}
