# xmip-core-transport-io-link

IO-Link transport: IEC 61131-9 over a serial line — M-sequences of type 0, 1 and 2, ISDU read and write of parameters, and the process data cycle; a Stream travels as one ISDU. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

A send target is read by `net::Target` in [xmip-core-library-net](https://github.com/IlleNilsson/xmip-core-library-net), the one reading of a URI every technology calls: scheme, authority, path and decoded query. Until 2026-09-28 it was read through the transport capability's `socket::target`, which split it on its first slash and left the query in the path.

A Send Location writes the parameter its `index` and `subindex` settings name, through the port its address names: it sends to the empty target (`Configured::own_target`, `transport::destination::OWN`), which it reads, through `transport::destination::named`, as that parameter. A target naming another still wins — `0x0300/1` (ADR-0064, amendment 2026-11-10).

Every M-sequence is written and its answer read through the one port its
address names, which `xmip-core-transport-serial` keeps open between the two,
so a device's answer that comes before the read is read.

Every IO-Link Location on a node naming one port goes through that port's
one handle (ADR-0064, amendment 2026-11-15): the master takes it from what
the node's Locations share (`Configured::shared_by`), so a Send and a Receive
Location on one device both reach it, where until 2026-11-15 the second was
refused the port. The port is held for an M-sequence whole, and for an ISDU
transfer whole (`serial::Exchange`), as another Location's M-sequence inside
one would reach the device as part of it.

A `0x` number in a target is read by `codec::hex::prefixed_number` in [xmip-core-library-codec](https://github.com/IlleNilsson/xmip-core-library-codec), which refuses a sign; until 2026-09-28 it was read with `from_str_radix`, which took `0x+7e8`.

A device at the far end of a real line reads each master message by the length its first two bytes tell, the control's read bit and the type, where no process data out rides (`MasterMessage::measure`), and says when it holds no ISDU in hand, between two transfers (`Device::idle`). The root's service test stands one so on a pseudo-terminal and a node's IO-Link Send Location writes a Stream to it (the owner, 2026-11-25: *just do your best for now*). The serial dependency here leaves out serial's port, so a build that links IO-Link brings it.

## Acknowledgement

A receive is an ISDU read of the parameter, which consumes nothing at the
device. Its verdict therefore has nothing to tell the device, whichever it is:
a receive cycle that did not complete loses nothing, and the next read finds
the parameter again. The parameter's bytes arrive whole.

## The declaration

What a Location of this technology is configured with is two `const`s in its `declaration` module, `src/declaration.rs`, beside the code, which `Configured` reads (ADR-0064, amendment 2026-11-26): `declaration::SETTINGS`, `rate`, `index`, `subindex` and `timeout`, and `SCHEMES`, `iolink`. The parameter a Stream travels as and the wait `timeout` defaults to, `STREAM_PARAMETER` and `DEFAULT_TIMEOUT`, are there too, and the code reads them from there; `STREAM_PARAMETER` is at the crate's root as well. It is the same on every operating system, one with no serial port included. Built with its `declaration` feature alone (`--no-default-features --features declaration`), the crate is those and nothing more: its code and every crate the code is built on — the serial technology it rides on among them, its port with it — are behind the default feature. So the library the surfaces load carries it without the code, through the gathering crate xmip-core-declaration (open problem 48), and the language server and the desktop suggest and check an IO-Link Location's settings though no node in their process loaded the technology. `test/Declaration.Test.ps1` in the estate holds that the declaration builds nothing the transport capability does not.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
