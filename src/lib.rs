#![forbid(unsafe_code)]

//! Streams that are parameters of an IO-Link device. One parameter — an
//! index and a subindex — is one Stream: reading it is an ISDU read,
//! writing it an ISDU write, and a Stream travels as one ISDU, which is
//! why [`DATA_MAX`] is the ceiling.
//!
//! IO-Link is the last metre of the plant: one sensor or actuator on one
//! three-wire cable to one master port, a 24 volt signal driven at up to
//! 230 kilobaud. What is here is the M-sequence — the one shape everything
//! on the line has ([`m_sequence`]) — the indexed service data unit and its
//! flow control ([`isdu`]), a master that speaks both and cycles process
//! data, and [`Device`], a device at the far end of an in-process line for
//! tests and the loopback. The carrier is `xmip-core-transport-serial`:
//! every M-sequence is a fixed-length frame on its line, both ends knowing
//! from the type how long each message is, and the loopback line is the
//! serial technology's own.
//!
//! The origin URI names the port and the parameter:
//! `iolink://<port>/0x0100/0`. A target is the same, or a bare
//! `0x<index>/<sub>`, or nothing for the configured parameter.

pub mod device;
pub mod isdu;
pub mod m_sequence;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub use device::Device;
pub use isdu::{DATA_MAX, Isdu};
pub use m_sequence::{Channel, DeviceMessage, Kind, MasterMessage};
use serial::{Framing, SerialTransport};
use transport::error::{Result, protocol_error};
use transport::loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback};
use transport::{Arrived, Directions, Transport};

use crate::isdu::Service;

/// The parameter a Stream travels as unless a target says otherwise: the
/// first vendor-specific index.
pub const STREAM_PARAMETER: (u16, u8) = (0x0100, 0);

/// The master's side of one port, speaking to the device on it.
#[derive(Clone)]
pub struct IoLinkTransport {
    line: SerialTransport,
    device: Option<Arc<Mutex<Device>>>,
    process_in: usize,
    index: u16,
    subindex: u8,
    timeout: Duration,
}

impl IoLinkTransport {
    /// A master on `line`, no process data, about [`STREAM_PARAMETER`].
    #[must_use]
    pub const fn new(line: SerialTransport) -> Self {
        Self {
            line,
            device: None,
            process_in: 0,
            index: STREAM_PARAMETER.0,
            subindex: STREAM_PARAMETER.1,
            timeout: Duration::from_secs(1),
        }
    }

    /// The device presents `process_in` bytes of process data in.
    #[must_use]
    pub const fn with_process_in(mut self, process_in: usize) -> Self {
        self.process_in = process_in;
        self
    }

    /// Speak about `index:subindex` instead.
    #[must_use]
    pub const fn about(mut self, index: u16, subindex: u8) -> Self {
        self.index = index;
        self.subindex = subindex;
        self
    }

    /// Give up on a device that stays busy.
    #[must_use]
    pub const fn timing_out_after(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// `iolink://<port>/0x<index>/<sub>`.
    #[must_use]
    pub fn origin(&self, index: u16, subindex: u8) -> String {
        let port = self.line.origin();
        let port = port
            .strip_prefix("serial://")
            .unwrap_or(&port)
            .split('?')
            .next()
            .unwrap_or_default();
        format!("iolink://{port}/{index:#06x}/{subindex}")
    }

    /// One M-sequence: the master message on the line, the device message
    /// off it.
    ///
    /// # Errors
    /// A line that could not be written or read, or a device message that
    /// is not one.
    pub fn exchange(&self, master: &MasterMessage) -> Result<DeviceMessage> {
        let bytes = master.encode();
        self.write_line(&bytes)?;
        if let Some(device) = &self.device {
            let heard = self.read_line(bytes.len())?;
            let heard = MasterMessage::decode(&heard, master.process_out.len())?;
            let answer = device
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .answer(&heard);
            self.write_line(&answer.encode())?;
        }
        let process_in = if master.process_out.is_empty() {
            0
        } else {
            self.process_in
        };
        let want = if master.read {
            master.kind.on_request()
        } else {
            0
        };
        let answer = self.read_line(DeviceMessage::length(process_in, want))?;
        DeviceMessage::decode(&answer, process_in)
    }

    /// One byte of the direct parameter page.
    ///
    /// # Errors
    /// As [`Self::exchange`].
    pub fn read_page(&self, address: u8) -> Result<u8> {
        let answer = self.exchange(&MasterMessage::read(Kind::Type0, Channel::Page, address))?;
        answer
            .on_request
            .first()
            .copied()
            .ok_or_else(|| protocol_error("a page read answered with no byte"))
    }

    /// One process data cycle: `process_out` to the device, its process
    /// data in back.
    ///
    /// # Errors
    /// As [`Self::exchange`], or process data the device says is not valid.
    pub fn cycle(&self, process_out: &[u8]) -> Result<Vec<u8>> {
        let master =
            MasterMessage::read(Kind::Type2, Channel::Process, 0).with_process(process_out);
        let answer = self.exchange(&master)?;
        if answer.process_invalid {
            return Err(protocol_error(
                "the device says its process data is not valid",
            ));
        }
        Ok(answer.process_in)
    }

    /// One ISDU transfer: `request` across the channel two bytes at a
    /// time, the response back the same way.
    ///
    /// # Errors
    /// A device that stays busy past the timeout, or answers what is not
    /// an ISDU.
    pub fn transfer(&self, request: &Isdu) -> Result<Isdu> {
        let bytes = request.encode();
        let mut address = isdu::START;
        for pair in bytes.chunks(2) {
            let mut two = pair.to_vec();
            two.resize(2, 0);
            self.exchange(&MasterMessage::write(
                Kind::Type1,
                Channel::Isdu,
                address,
                &two,
            )?)?;
            address = (address + 1) & isdu::COUNT_MASK;
        }
        let deadline = Instant::now() + self.timeout;
        let mut response = loop {
            let first = self
                .exchange(&MasterMessage::read(
                    Kind::Type1,
                    Channel::Isdu,
                    isdu::START,
                ))?
                .on_request;
            if first.first() != Some(&isdu::BUSY) {
                break first;
            }
            if Instant::now() >= deadline {
                return Err(protocol_error("the device stayed busy past the deadline"));
            }
        };
        let length =
            Isdu::length_of(&response).ok_or_else(|| protocol_error("an ISDU cut short"))?;
        let mut address = 0;
        while response.len() < length {
            let more = self
                .exchange(&MasterMessage::read(Kind::Type1, Channel::Isdu, address))?
                .on_request;
            response.extend_from_slice(&more);
            address = (address + 1) & isdu::COUNT_MASK;
        }
        response.truncate(length);
        Isdu::decode(&response)
    }

    /// Write `bytes` to parameter `index:subindex`.
    ///
    /// # Errors
    /// Over [`DATA_MAX`], or a device that answers negatively.
    pub fn write_parameter(&self, index: u16, subindex: u8, bytes: &[u8]) -> Result<()> {
        let answer = self.transfer(&Isdu::write(index, subindex, bytes)?)?;
        match answer.service {
            Service::WriteResponsePositive => Ok(()),
            other => Err(refused(other, &answer.data)),
        }
    }

    /// Read parameter `index:subindex`.
    ///
    /// # Errors
    /// A device that answers negatively.
    pub fn read_parameter(&self, index: u16, subindex: u8) -> Result<Vec<u8>> {
        let answer = self.transfer(&Isdu::read(index, subindex))?;
        match answer.service {
            Service::ReadResponsePositive => Ok(answer.data),
            other => Err(refused(other, &answer.data)),
        }
    }

    fn write_line(&self, bytes: &[u8]) -> Result<()> {
        self.line
            .clone()
            .framed(Framing::Fixed(bytes.len()))
            .send("", bytes)
    }

    fn read_line(&self, length: usize) -> Result<Vec<u8>> {
        self.line
            .clone()
            .framed(Framing::Fixed(length))
            .receive()?
            .into_iter()
            .next()
            .map(|arrived| arrived.bytes)
            .ok_or_else(|| protocol_error("nothing came off the line"))
    }

    /// The parameter a target names, or the configured one.
    fn resolve(&self, target: &str) -> Result<(u16, u8)> {
        let path = match transport::socket::target("iolink", target) {
            Some((_, path)) => path,
            None => target,
        };
        if path.is_empty() {
            return Ok((self.index, self.subindex));
        }
        let bad = || protocol_error(format!("{target:?} is not 0x<index>/<sub>"));
        let (index, subindex) = path.split_once('/').ok_or_else(bad)?;
        let index = index
            .strip_prefix("0x")
            .and_then(|hex| u16::from_str_radix(hex, 16).ok())
            .ok_or_else(bad)?;
        Ok((index, subindex.parse().map_err(|_| bad())?))
    }
}

fn refused(service: Service, data: &[u8]) -> transport::TransportError {
    protocol_error(format!("the device answered {service:?} with {data:02x?}"))
}

impl Transport for IoLinkTransport {
    fn name(&self) -> &'static str {
        "io-link"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    /// One read of the parameter: its bytes as one Stream.
    fn receive(&self) -> Result<Vec<Arrived>> {
        let bytes = self.read_parameter(self.index, self.subindex)?;
        Ok(vec![Arrived::new(
            self.origin(self.index, self.subindex),
            bytes,
        )])
    }

    /// One write of `bytes` to the parameter `target` names.
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let (index, subindex) = self.resolve(target)?;
        self.write_parameter(index, subindex, bytes)
    }
}

impl IoLinkTransport {
    /// Both ends on the serial technology's in-process line: a master and a
    /// device holding the Stream parameter empty, the loopback timeout on
    /// the master.
    #[must_use]
    pub fn loopback() -> Self {
        let device =
            Device::new(0).with_parameter(STREAM_PARAMETER.0, STREAM_PARAMETER.1, Vec::new());
        Self::new(SerialTransport::loopback())
            .attached(device)
            .timing_out_after(LOOPBACK_TIMEOUT)
    }

    /// The same master with `device` at the far end of its line, answering
    /// every M-sequence as it is written.
    #[must_use]
    pub fn attached(mut self, device: Device) -> Self {
        self.device = Some(Arc::new(Mutex::new(device)));
        self
    }
}

/// The device holding what the master wrote, until it is read back.
struct Holding {
    master: IoLinkTransport,
    address: String,
}

impl FarEnd for Holding {
    fn address(&self) -> &str {
        &self.address
    }

    fn take_one(self: Box<Self>) -> Result<Arrived> {
        self.master
            .receive()?
            .into_iter()
            .next()
            .ok_or_else(|| protocol_error("nothing came back from the device"))
    }
}

impl Loopback for IoLinkTransport {
    /// A Stream travels as one ISDU, and an ISDU is at most 238 bytes with
    /// its framing.
    fn ceiling(&self) -> Option<usize> {
        Some(DATA_MAX)
    }

    fn far_end(&self) -> Result<Box<dyn FarEnd>> {
        if self.device.is_none() {
            return Err(protocol_error("a port with no device attached in-process"));
        }
        Ok(Box::new(Holding {
            master: self.clone(),
            address: self.origin(self.index, self.subindex),
        }))
    }

    fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
        self.send(address, payload)
    }

    fn unblock(&self, _address: &str) {}

    /// In order on one thread: the device answers as the master writes, so
    /// the write goes first and the read-back finds what it left.
    fn round(&self, payload: &[u8]) -> Result<Arrived> {
        let far = self.far_end()?;
        self.send_to(far.address(), payload)?;
        let arrived = far.take_one()?;
        if arrived.bytes != payload {
            return Err(protocol_error("written, but what was read back differs"));
        }
        Ok(arrived)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes a protocol breaks on, as the Playground lists them, cut
    /// to the ceiling where they are longer.
    fn edge_payloads() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("empty", Vec::new()),
            ("one byte", vec![0x2a]),
            (
                "every byte",
                (0..DATA_MAX)
                    .map(|n| u8::try_from(n).unwrap_or(0))
                    .collect(),
            ),
            ("nul run", vec![0; DATA_MAX]),
            ("high bytes", vec![0xff; DATA_MAX]),
            ("crlf storm", b"\r\n".repeat(DATA_MAX / 2)),
        ]
    }

    #[test]
    fn a_loopback_round_writes_a_parameter_as_one_isdu_and_reads_it_back() {
        let loopback = IoLinkTransport::loopback();
        let arrived = loopback.round(b"one ISDU").expect("round");
        assert_eq!(arrived.bytes, b"one ISDU");
        assert_eq!(arrived.origin_uri, "iolink://loopback/0x0100/0");
        assert_eq!(loopback.ceiling(), Some(232));
        assert!(loopback.refuses(b"one ISDU").is_none());
        assert_eq!(loopback.name(), "io-link");
        assert!(loopback.claims().is_none());
    }

    #[test]
    fn the_loopback_returns_the_edges_whole_and_refuses_over_the_brim() {
        let loopback = IoLinkTransport::loopback();
        for (name, bytes) in edge_payloads() {
            let arrived = loopback
                .round(&bytes)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(arrived.bytes, bytes, "{name}");
        }
        let error = loopback
            .round(&[0; DATA_MAX + 1])
            .expect_err("over the brim");
        assert!(error.message.contains("over the 232"), "{error}");
    }

    #[test]
    fn a_target_names_the_parameter_and_a_missing_one_is_refused() {
        let device = Device::new(2)
            .with_parameter(0x0200, 1, vec![0])
            .with_page(0x07, &[0x01, 0x23]);
        let master = IoLinkTransport::new(SerialTransport::loopback())
            .attached(device)
            .with_process_in(2)
            .timing_out_after(Duration::from_millis(100));
        master
            .send("iolink://loopback/0x0200/1", &[1, 2, 3])
            .expect("another parameter");
        assert_eq!(
            master.clone().about(0x0200, 1).receive().expect("read")[0].bytes,
            [1, 2, 3]
        );
        master.send("0x0200/1", &[]).expect("bare target, empty");
        assert!(
            master.clone().about(0x0200, 1).receive().expect("read")[0]
                .bytes
                .is_empty()
        );
        let error = master.send("", b"x").expect_err("no such parameter");
        assert!(error.message.contains("WriteResponseNegative"), "{error}");
        assert!(master.receive().is_err(), "no such parameter to read");
        assert!(master.send("0200/1", b"x").is_err(), "not hex");
        assert_eq!(master.read_page(0x08).expect("page"), 0x23);
        assert_eq!(master.cycle(&[9, 9]).expect("cycle"), [0, 0]);
    }

    #[test]
    fn a_port_with_no_device_in_process_has_no_far_end() {
        let master = IoLinkTransport::new(SerialTransport::loopback());
        assert!(master.far_end().is_err());
        assert!(master.receive().is_err(), "nobody answers an ISDU");
    }
}
