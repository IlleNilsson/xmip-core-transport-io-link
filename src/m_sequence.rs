//! The M-sequence, IEC 61131-9 clause 7.2: one master message and the
//! device message that answers it, the whole of what passes on an IO-Link
//! line. Nothing else ever does.
//!
//! The master message opens with the M-sequence control byte — read or
//! write, which of four channels, and an address — and the checksum and
//! type byte, then process data out and on-request data to write. The
//! device answers process data in, on-request data read, and its own
//! checksum and status byte. Type 0 carries one on-request byte, type 1
//! two, type 2 one beside the process data. The checksum is the eight-bit
//! XOR seeded with `0x52`, folded to six bits as the standard folds it.

use transport::error::{Result, protocol_error};

/// The checksum seed of every message.
const SEED: u8 = 0x52;

/// Which of the four channels an on-request byte is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Process = 0,
    Page = 1,
    Diagnosis = 2,
    Isdu = 3,
}

impl Channel {
    fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0 => Self::Process,
            1 => Self::Page,
            2 => Self::Diagnosis,
            _ => Self::Isdu,
        }
    }
}

/// The M-sequence type: how many on-request bytes ride, and whether
/// process data does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Type0 = 0,
    Type1 = 1,
    Type2 = 2,
}

impl Kind {
    /// The on-request bytes this type carries each way.
    #[must_use]
    pub const fn on_request(self) -> usize {
        match self {
            Self::Type0 | Self::Type2 => 1,
            Self::Type1 => 2,
        }
    }
}

/// The master's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasterMessage {
    pub read: bool,
    pub channel: Channel,
    pub address: u8,
    pub kind: Kind,
    /// Process data out: type 2 only, empty otherwise.
    pub process_out: Vec<u8>,
    /// On-request data to write: the type's count on a write, empty on a read.
    pub on_request: Vec<u8>,
}

/// The device's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceMessage {
    /// Process data in: type 2 only, empty otherwise.
    pub process_in: Vec<u8>,
    /// On-request data read: the type's count on a read, empty on a write.
    pub on_request: Vec<u8>,
    /// The device has an event pending.
    pub event: bool,
    /// The process data in is not valid.
    pub process_invalid: bool,
}

/// The eight-bit XOR of `bytes` under the seed, folded to six bits.
#[must_use]
pub fn checksum(bytes: &[u8]) -> u8 {
    let eight = bytes.iter().fold(SEED, |acc, byte| acc ^ byte);
    let bit = |n: u8| (eight >> n) & 1;
    let d5 = bit(7) ^ bit(5) ^ bit(3) ^ bit(1);
    let d4 = bit(6) ^ bit(4) ^ bit(2) ^ bit(0);
    let d3 = bit(7) ^ bit(6);
    let d2 = bit(5) ^ bit(4);
    let d1 = bit(3) ^ bit(2);
    let d0 = bit(1) ^ bit(0);
    (d5 << 5) | (d4 << 4) | (d3 << 3) | (d2 << 2) | (d1 << 1) | d0
}

impl MasterMessage {
    /// A read of `address` on `channel`.
    #[must_use]
    pub const fn read(kind: Kind, channel: Channel, address: u8) -> Self {
        Self {
            read: true,
            channel,
            address: address & 0x1f,
            kind,
            process_out: Vec::new(),
            on_request: Vec::new(),
        }
    }

    /// A write of `on_request` to `address` on `channel`.
    ///
    /// # Errors
    /// On-request data that is not the type's count.
    pub fn write(kind: Kind, channel: Channel, address: u8, on_request: &[u8]) -> Result<Self> {
        if on_request.len() != kind.on_request() {
            return Err(protocol_error(format!(
                "{} on-request bytes on a type carrying {}",
                on_request.len(),
                kind.on_request()
            )));
        }
        Ok(Self {
            read: false,
            channel,
            address: address & 0x1f,
            kind,
            process_out: Vec::new(),
            on_request: on_request.to_vec(),
        })
    }

    /// The same message with `process_out` beside it, as type 2 carries.
    #[must_use]
    pub fn with_process(mut self, process_out: &[u8]) -> Self {
        self.process_out = process_out.to_vec();
        self
    }

    /// The bytes on the line, checksum in place.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let control = (u8::from(self.read) << 7) | ((self.channel as u8) << 5) | self.address;
        let mut out = vec![control, (self.kind as u8) << 6];
        out.extend_from_slice(&self.process_out);
        out.extend_from_slice(&self.on_request);
        out[1] |= checksum(&out);
        out
    }

    /// How long the master message is on the line, for a `kind` with
    /// `process_out` bytes of process data out and a `read` or a write.
    #[must_use]
    pub const fn length(kind: Kind, process_out: usize, read: bool) -> usize {
        2 + process_out + if read { 0 } else { kind.on_request() }
    }

    /// The message `bytes` carry, `process_out` bytes of process data out
    /// expected before the on-request data.
    ///
    /// # Errors
    /// Shorter than two bytes, a checksum that does not match, or a length
    /// that is not the type's.
    pub fn decode(bytes: &[u8], process_out: usize) -> Result<Self> {
        if bytes.len() < 2 {
            return Err(protocol_error("a master message shorter than two bytes"));
        }
        let mut check = bytes.to_vec();
        check[1] &= 0xc0;
        if checksum(&check) != bytes[1] & 0x3f {
            return Err(protocol_error(
                "a master message whose checksum does not match",
            ));
        }
        let read = bytes[0] & 0x80 != 0;
        let kind = match bytes[1] >> 6 {
            0 => Kind::Type0,
            1 => Kind::Type1,
            2 => Kind::Type2,
            _ => return Err(protocol_error("M-sequence type 3 is reserved")),
        };
        if bytes.len() != Self::length(kind, process_out, read) {
            return Err(protocol_error(
                "a master message that is not its type's length",
            ));
        }
        Ok(Self {
            read,
            channel: Channel::from_bits(bytes[0] >> 5),
            address: bytes[0] & 0x1f,
            kind,
            process_out: bytes[2..2 + process_out].to_vec(),
            on_request: bytes[2 + process_out..].to_vec(),
        })
    }
}

impl DeviceMessage {
    /// The bytes on the line, checksum and status in place.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.process_in.len() + self.on_request.len() + 1);
        out.extend_from_slice(&self.process_in);
        out.extend_from_slice(&self.on_request);
        out.push((u8::from(self.event) << 7) | (u8::from(self.process_invalid) << 6));
        let check = checksum(&out);
        if let Some(last) = out.last_mut() {
            *last |= check;
        }
        out
    }

    /// How long the device message is on the line, answering `kind` with
    /// `process_in` bytes of process data in and `on_request` bytes read.
    #[must_use]
    pub const fn length(process_in: usize, on_request: usize) -> usize {
        process_in + on_request + 1
    }

    /// The message `bytes` carry, `process_in` bytes of process data in
    /// before the on-request data.
    ///
    /// # Errors
    /// Empty, shorter than its process data, or a checksum that does not
    /// match.
    pub fn decode(bytes: &[u8], process_in: usize) -> Result<Self> {
        let (&status, body) = bytes
            .split_last()
            .ok_or_else(|| protocol_error("an empty device message"))?;
        let mut check = body.to_vec();
        check.push(status & 0xc0);
        if checksum(&check) != status & 0x3f {
            return Err(protocol_error(
                "a device message whose checksum does not match",
            ));
        }
        if body.len() < process_in {
            return Err(protocol_error(
                "a device message shorter than its process data",
            ));
        }
        Ok(Self {
            process_in: body[..process_in].to_vec(),
            on_request: body[process_in..].to_vec(),
            event: status & 0x80 != 0,
            process_invalid: status & 0x40 != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_1_write_and_its_answer_round_trip_with_checksums() {
        let master =
            MasterMessage::write(Kind::Type1, Channel::Isdu, 0x10, &[0x31, 0x01]).expect("m");
        let wire = master.encode();
        assert_eq!(wire.len(), 4);
        assert_eq!(wire[0], 0x70, "write, ISDU channel, address 0x10");
        assert_eq!(wire[1] >> 6, 1, "type 1");
        assert_eq!(MasterMessage::decode(&wire, 0).expect("decode"), master);
        let device = DeviceMessage {
            process_in: Vec::new(),
            on_request: Vec::new(),
            event: true,
            process_invalid: false,
        };
        let wire = device.encode();
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0] & 0xc0, 0x80);
        assert_eq!(DeviceMessage::decode(&wire, 0).expect("decode"), device);
    }

    #[test]
    fn a_type_2_read_carries_process_data_both_ways() {
        let master =
            MasterMessage::read(Kind::Type2, Channel::Page, 0x02).with_process(&[0xaa, 0xbb]);
        let wire = master.encode();
        assert_eq!(wire.len(), MasterMessage::length(Kind::Type2, 2, true));
        assert_eq!(wire[0], 0xa2, "read, page channel, address 2");
        assert_eq!(MasterMessage::decode(&wire, 2).expect("decode"), master);
        let device = DeviceMessage {
            process_in: vec![1, 2, 3, 4],
            on_request: vec![0x11],
            event: false,
            process_invalid: true,
        };
        let wire = device.encode();
        assert_eq!(wire.len(), DeviceMessage::length(4, 1));
        assert_eq!(DeviceMessage::decode(&wire, 4).expect("decode"), device);
        assert_eq!(Kind::Type1.on_request(), 2);
        assert_eq!(Kind::Type2.on_request(), 1);
    }

    #[test]
    fn what_is_not_an_m_sequence_is_refused() {
        assert!(MasterMessage::decode(&[0x80], 0).is_err(), "short");
        let mut wire = MasterMessage::read(Kind::Type0, Channel::Page, 1).encode();
        wire[0] ^= 0x01;
        assert!(MasterMessage::decode(&wire, 0).is_err(), "checksum");
        let wire = MasterMessage::read(Kind::Type0, Channel::Page, 1).encode();
        assert!(
            MasterMessage::decode(&wire, 1).is_err(),
            "not the type's length"
        );
        let mut reserved = vec![0x81, 0xc0];
        reserved[1] |= checksum(&reserved);
        assert!(MasterMessage::decode(&reserved, 0).is_err(), "type 3");
        assert!(MasterMessage::write(Kind::Type0, Channel::Isdu, 0, &[1, 2]).is_err());
        assert!(DeviceMessage::decode(&[], 0).is_err(), "empty");
        let mut wire = DeviceMessage {
            process_in: vec![9],
            on_request: Vec::new(),
            event: false,
            process_invalid: false,
        }
        .encode();
        assert!(
            DeviceMessage::decode(&wire, 4).is_err(),
            "shorter than its process data"
        );
        wire[0] ^= 0x01;
        assert!(DeviceMessage::decode(&wire, 1).is_err(), "checksum");
    }
}
