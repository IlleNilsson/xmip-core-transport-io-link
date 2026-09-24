//! The indexed service data unit, IEC 61131-9 clause 7.3.3: a read or a
//! write of one parameter by index and subindex, up to 238 bytes in all,
//! carried across the ISDU channel two on-request bytes at a time under
//! the flow control the master's address field spells out.
//!
//! The first byte is the I-Service in its high nibble and the length in
//! its low one, where a length of one says an extended length byte
//! follows; then the index, the subindex, the data, and a checksum byte
//! that makes the XOR of the whole unit zero. The master starts a transfer
//! with `START`, counts the bytes it sends, and reads the answer back the
//! same way; a device still working answers `BUSY`.

use transport::ceiling;
use transport::error::{Result, protocol_error};

/// The longest ISDU there is: what the extended length byte can name.
pub const MAX_LENGTH: usize = 238;

/// The I-Service, `ExtLength`, two bytes of index, subindex and checksum of
/// a 16-bit write or read.
const OVERHEAD: usize = 6;

/// The data one ISDU carries: the longest, less its framing.
pub const DATA_MAX: usize = MAX_LENGTH - OVERHEAD;

/// The flow control in the address field of an ISDU-channel message.
pub const START: u8 = 0x10;
pub const IDLE: u8 = 0x11;
pub const ABORT: u8 = 0x1f;
/// A count is the low four bits, `0x00` to `0x0f`, cycling.
pub const COUNT_MASK: u8 = 0x0f;

/// What a device answers on the ISDU channel while it is still working,
/// and when it has no service to answer.
pub const BUSY: u8 = 0x01;
pub const NO_SERVICE: u8 = 0x00;

/// The I-Service codes this crate speaks: 16-bit index with subindex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    WriteRequest = 0x3,
    WriteResponseNegative = 0x4,
    WriteResponsePositive = 0x5,
    ReadRequest = 0xb,
    ReadResponseNegative = 0xc,
    ReadResponsePositive = 0xd,
}

impl Service {
    fn from_nibble(nibble: u8) -> Result<Self> {
        Ok(match nibble {
            0x3 => Self::WriteRequest,
            0x4 => Self::WriteResponseNegative,
            0x5 => Self::WriteResponsePositive,
            0xb => Self::ReadRequest,
            0xc => Self::ReadResponseNegative,
            0xd => Self::ReadResponsePositive,
            other => {
                return Err(protocol_error(format!(
                    "I-Service {other:#x} is not one this transport speaks"
                )));
            }
        })
    }

    /// True for a response, which carries no index.
    #[must_use]
    pub const fn is_response(self) -> bool {
        !matches!(self, Self::WriteRequest | Self::ReadRequest)
    }
}

/// One ISDU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Isdu {
    pub service: Service,
    /// The index and subindex: a request's; zero on a response.
    pub index: u16,
    pub subindex: u8,
    pub data: Vec<u8>,
}

impl Isdu {
    /// A write of `data` to `index:subindex`.
    ///
    /// # Errors
    /// Data over [`DATA_MAX`].
    pub fn write(index: u16, subindex: u8, data: &[u8]) -> Result<Self> {
        Self::new(Service::WriteRequest, index, subindex, data)
    }

    /// A read of `index:subindex`.
    #[must_use]
    pub const fn read(index: u16, subindex: u8) -> Self {
        Self {
            service: Service::ReadRequest,
            index,
            subindex,
            data: Vec::new(),
        }
    }

    /// A response carrying `data`.
    ///
    /// # Errors
    /// Data over [`DATA_MAX`].
    pub fn response(service: Service, data: &[u8]) -> Result<Self> {
        Self::new(service, 0, 0, data)
    }

    fn new(service: Service, index: u16, subindex: u8, data: &[u8]) -> Result<Self> {
        ceiling::within(data.len(), DATA_MAX, "one ISDU carries")?;
        Ok(Self {
            service,
            index,
            subindex,
            data: data.to_vec(),
        })
    }

    /// The unit's bytes, checksum in place.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![(self.service as u8) << 4 | 0x1, 0];
        if !self.service.is_response() {
            out.extend_from_slice(&self.index.to_be_bytes());
            out.push(self.subindex);
        }
        out.extend_from_slice(&self.data);
        out.push(0);
        out[1] = u8::try_from(out.len()).unwrap_or(u8::MAX);
        let check = out.iter().fold(0u8, |acc, byte| acc ^ byte);
        if let Some(last) = out.last_mut() {
            *last = check;
        }
        out
    }

    /// The whole length of the unit the first two bytes of `head` name, or
    /// `None` where fewer than two bytes are there yet.
    #[must_use]
    pub fn length_of(head: &[u8]) -> Option<usize> {
        let first = *head.first()?;
        match first & 0x0f {
            1 => head.get(1).map(|&ext| usize::from(ext)),
            n => Some(usize::from(n)),
        }
    }

    /// The unit `bytes` carry.
    ///
    /// # Errors
    /// A length that is not the bytes present, a checksum that is not zero,
    /// or a service this transport does not speak.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let length = Self::length_of(bytes).ok_or_else(|| protocol_error("an ISDU cut short"))?;
        if length != bytes.len() || length < 3 {
            return Err(protocol_error("an ISDU whose length is not its bytes"));
        }
        if bytes.iter().fold(0u8, |acc, byte| acc ^ byte) != 0 {
            return Err(protocol_error("an ISDU whose checksum is not zero"));
        }
        let service = Service::from_nibble(bytes[0] >> 4)?;
        let at = if bytes[0] & 0x0f == 1 { 2 } else { 1 };
        let body = &bytes[at..bytes.len() - 1];
        if service.is_response() {
            return Ok(Self {
                service,
                index: 0,
                subindex: 0,
                data: body.to_vec(),
            });
        }
        let (head, data) = body
            .split_at_checked(3)
            .ok_or_else(|| protocol_error("a request without its index"))?;
        Ok(Self {
            service,
            index: u16::from_be_bytes([head[0], head[1]]),
            subindex: head[2],
            data: data.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_request_encodes_its_service_length_index_and_checksum() {
        let write = Isdu::write(0x0100, 0, &[1, 2, 3]).expect("write");
        let wire = write.encode();
        assert_eq!(wire.len(), 9);
        assert_eq!(&wire[..5], &[0x31, 9, 0x01, 0x00, 0]);
        assert_eq!(wire.iter().fold(0u8, |acc, byte| acc ^ byte), 0);
        assert_eq!(Isdu::length_of(&wire), Some(9));
        assert_eq!(Isdu::decode(&wire).expect("decode"), write);
        let read = Isdu::read(0x0010, 2);
        assert_eq!(Isdu::decode(&read.encode()).expect("decode"), read);
        let ok = Isdu::response(Service::WriteResponsePositive, &[]).expect("ok");
        assert_eq!(ok.encode(), [0x51, 3, 0x51 ^ 3]);
        assert_eq!(Isdu::decode(&ok.encode()).expect("decode"), ok);
        let brim = Isdu::write(0, 0, &[9; DATA_MAX]).expect("brim");
        assert_eq!(brim.encode().len(), MAX_LENGTH);
        assert_eq!(Isdu::decode(&brim.encode()).expect("decode"), brim);
        assert!(
            Isdu::write(0, 0, &[0; DATA_MAX + 1]).is_err(),
            "over the brim"
        );
    }

    #[test]
    fn what_is_not_an_isdu_is_refused() {
        assert!(Isdu::length_of(&[]).is_none());
        assert!(
            Isdu::length_of(&[0x31]).is_none(),
            "extended length not there yet"
        );
        assert!(Isdu::decode(&[]).is_err(), "empty");
        let mut wire = Isdu::write(1, 0, &[7]).expect("w").encode();
        wire[4] ^= 1;
        assert!(Isdu::decode(&wire).is_err(), "checksum");
        let wire = Isdu::write(1, 0, &[7]).expect("w").encode();
        assert!(Isdu::decode(&wire[..5]).is_err(), "length not its bytes");
        let mut unknown = Isdu::read(1, 0).encode();
        unknown[0] = 0x91;
        let check = unknown[..unknown.len() - 1]
            .iter()
            .fold(0u8, |acc, b| acc ^ b);
        let last = unknown.len() - 1;
        unknown[last] = check;
        assert!(Isdu::decode(&unknown).is_err(), "8-bit index service");
        let short = [0x33, 0x33];
        assert!(Isdu::decode(&short).is_err(), "no index");
    }
}
