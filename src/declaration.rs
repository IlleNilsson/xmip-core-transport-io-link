//! This technology's declaration: the transport guide's "The declaration",
//! `module/core/capability/transport/doc/adding-a-transport.md`. It is the
//! same on every operating system, one with no serial port included: a
//! Location's settings are suggested and checked wherever it is designed.

use std::time::Duration;

use xcore::settings::{Applies, Fixed, Kind, Presence, Setting, Settings};

/// The parameter a Stream travels as unless a target says otherwise: the
/// first vendor-specific index.
pub const STREAM_PARAMETER: (u16, u8) = (0x0100, 0);

/// How long a master waits on a device that stays busy unless a Location
/// says.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);

/// The transmission rates IO-Link names, and the baud each is: the `rate`
/// setting's choices and what the code opens the line at, written once.
pub(crate) const RATES: [(&str, u32); 3] = [("com1", 4_800), ("com2", 38_400), ("com3", 230_400)];

/// [`RATES`]' names, as the `rate` setting offers them.
const RATE_NAMES: [&str; 3] = [RATES[0].0, RATES[1].0, RATES[2].0];

/// Every setting a Location takes beyond its address, the serial port the
/// master's port drives.
pub const SETTINGS: &Settings = &Settings {
    technology: env!("CARGO_PKG_NAME"),
    settings: &[
        Setting {
            name: "rate",
            kind: Kind::Choice {
                choices: &RATE_NAMES,
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
        transport::configured::timeout(
            Applies::Both,
            Some(DEFAULT_TIMEOUT),
            "How long a device that stays busy is waited on.",
        ),
    ],
};

/// This technology's schemes, as [`transport::Configured::SCHEMES`] defines
/// them.
pub const SCHEMES: &[&str] = &["iolink"];
