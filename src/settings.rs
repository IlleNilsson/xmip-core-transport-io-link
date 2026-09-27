//! What an IO-Link Location is configured with, declared once and read
//! through (ADR-0064, amendment 2026-09-26).

use serial::SerialTransport;
use transport::Configured;
use transport::error::{Result, protocol_error};
use xcore::settings::{Applies, Fixed, Kind, Presence, Read, Setting, Settings};

use crate::{DEFAULT_TIMEOUT, IoLinkTransport, STREAM_PARAMETER};

/// The transmission rates IO-Link names, and the baud each is.
const RATES: [(&str, u32); 3] = [("com1", 4_800), ("com2", 38_400), ("com3", 230_400)];

impl Configured for IoLinkTransport {
    /// The address is the serial port the master's port drives.
    const SETTINGS: &'static Settings = &Settings {
        technology: env!("CARGO_PKG_NAME"),
        settings: &[
            Setting {
                name: "rate",
                kind: Kind::Choice {
                    choices: &["com1", "com2", "com3"],
                },
                presence: Presence::Required,
                meaning: "The transmission rate the device speaks: com1 is 4800 baud, com2 \
                          38400 and com3 230400.",
                applies: Applies::Both,
            },
            Setting {
                name: "index",
                kind: Kind::Integer {
                    minimum: 0,
                    maximum: 0xffff,
                },
                presence: Presence::Default(Fixed::Integer(STREAM_PARAMETER.0 as i64)),
                meaning: "The index of the parameter that is the Stream.",
                applies: Applies::Both,
            },
            Setting {
                name: "subindex",
                kind: Kind::Integer {
                    minimum: 0,
                    maximum: 0xff,
                },
                presence: Presence::Default(Fixed::Integer(STREAM_PARAMETER.1 as i64)),
                meaning: "The subindex of the parameter that is the Stream.",
                applies: Applies::Both,
            },
            Setting {
                name: "timeout",
                kind: Kind::Duration,
                presence: Presence::Default(Fixed::Duration(DEFAULT_TIMEOUT)),
                meaning: "How long a device that stays busy is waited on.",
                applies: Applies::Both,
            },
        ],
    };

    fn configured(address: &str, settings: &Read) -> Result<Self> {
        let rate = settings.text("rate");
        let baud = RATES
            .iter()
            .find_map(|(name, baud)| (*name == rate).then_some(*baud))
            .ok_or_else(|| protocol_error(format!("{rate:?} is not an IO-Link rate")))?;
        let index = u16::try_from(settings.integer("index"))
            .map_err(|_| protocol_error("an index is at most 0xffff"))?;
        let subindex = u8::try_from(settings.integer("subindex"))
            .map_err(|_| protocol_error("a subindex is at most 0xff"))?;
        Ok(Self::new(SerialTransport::new(address, baud))
            .about(index, subindex)
            .timing_out_after(settings.duration("timeout")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use xcore::settings::Given;

    #[test]
    fn io_link_declares_its_settings_and_reads_through_them() {
        assert_eq!(IoLinkTransport::SETTINGS.problems(), Vec::<String>::new());
        let given = [
            ("rate".to_string(), Given::Text("com3".to_string())),
            ("index".to_string(), Given::Integer(0x0200)),
            ("timeout".to_string(), Given::Text("500ms".to_string())),
        ];
        let built = IoLinkTransport::open("COM4", Applies::Receive, &given).expect("built");
        assert_eq!((built.index, built.subindex), (0x0200, STREAM_PARAMETER.1));
        assert_eq!(built.timeout, Duration::from_millis(500));
        assert_eq!(built.origin(1, 0), "iolink://COM4/0x0001/0");
        let given = [("rate".to_string(), Given::Text("com4".to_string()))];
        let Err(refused) = IoLinkTransport::open("COM4", Applies::Send, &given) else {
            panic!("com4 is not a rate");
        };
        assert!(refused.message.contains("\"rate\""), "{}", refused.message);
    }
}
