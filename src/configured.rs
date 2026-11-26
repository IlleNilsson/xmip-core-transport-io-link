//! How an IO-Link Location is configured: `Configured`, reading the
//! settings its [`declaration`](crate::declaration) declares once
//! (ADR-0064, amendments 2026-09-26 and 2026-11-26), and the port it takes
//! from what the Locations of its node share (amendment 2026-11-15).

use std::sync::Arc;

use serial::SerialTransport;
use transport::error::{Result, protocol_error};
use transport::{Configured, Sharing};
use xcore::settings::{Read, Settings};

use crate::IoLinkTransport;
use crate::declaration::{self, RATES};

impl Configured for IoLinkTransport {
    const SETTINGS: &'static Settings = declaration::SETTINGS;
    const SCHEMES: &'static [&'static str] = declaration::SCHEMES;

    /// The index and subindex settings: the address names only the port.
    fn own_target(_address: &str) -> &str {
        transport::destination::OWN
    }

    fn timeout_kept(&mut self) -> Option<&mut dyn transport::configured::TimeoutKept> {
        Some(&mut self.timeout)
    }

    /// The port every Location naming the master's port on the node goes
    /// through, so a Send and a Receive Location on one device each take
    /// whole turns on it.
    fn shared_by(self, sharing: &Arc<dyn Sharing>) -> Self {
        Self {
            line: self.line.shared_by(sharing),
            ..self
        }
    }

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
        Ok(Self::new(SerialTransport::new(address, baud)).about(index, subindex))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Device, STREAM_PARAMETER};
    use std::time::Duration;
    use transport::Transport;
    use xcore::settings::{Applies, Given};

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

    #[test]
    fn a_send_location_writes_its_parameter_and_a_target_naming_another_wins() {
        let given = [
            ("rate".to_string(), Given::Text("com3".to_string())),
            ("index".to_string(), Given::Integer(0x0200)),
        ];
        let location = "iolink://COM4";
        let mut near = IoLinkTransport::open(location, Applies::Send, &given).expect("built");
        // This machine has no COM4: the master the Location built goes on
        // the serial technology's in-process line, the far device on it.
        near.line = SerialTransport::loopback();
        let device = Device::new(0)
            .with_parameter(0x0200, STREAM_PARAMETER.1, Vec::new())
            .with_parameter(0x0300, 1, Vec::new());
        let near = near.attached(device);
        near.send(IoLinkTransport::own_target(location), b"own parameter")
            .expect("its own");
        near.send("0x0300/1", b"other").expect("another");
        let own = near.read_parameter(0x0200, STREAM_PARAMETER.1);
        assert_eq!(own.expect("its own"), b"own parameter");
        let other = near.read_parameter(0x0300, 1).expect("another");
        assert_eq!(other, b"other");
    }

    /// Two Locations on one master port take its one port from what their
    /// node shares, so what one writes on the line the other reads off it,
    /// where a port of its own reads nothing.
    #[test]
    fn locations_on_one_port_take_its_one_port_from_their_node() {
        use serial::Framing;
        use transport::loopback::LOOPBACK_TIMEOUT;
        let node: Arc<dyn Sharing> = Arc::new(transport::Shared::default());
        let first = IoLinkTransport::new(SerialTransport::loopback()).shared_by(&node);
        let second = IoLinkTransport::new(SerialTransport::loopback()).shared_by(&node);
        let alone = IoLinkTransport::new(SerialTransport::loopback());
        let framing = Framing::Fixed(3);
        let sent = first.line.exchanging().transmit(&framing, b"abc");
        sent.expect("sent");
        let read = alone.line.exchanging().receive(&framing, LOOPBACK_TIMEOUT);
        assert_eq!(read.expect("quiet"), None, "a port of its own");
        let read = second.line.exchanging().receive(&framing, LOOPBACK_TIMEOUT);
        assert_eq!(
            read.expect("read"),
            Some(b"abc".to_vec()),
            "one port for both"
        );
    }
}
