# xmip-core-transport-io-link

IO-Link transport: IEC 61131-9 over a serial line — M-sequences of type 0, 1 and 2, ISDU read and write of parameters, and the process data cycle; a Stream travels as one ISDU. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

A send target is read by `net::Target` in [xmip-core-library-net](https://github.com/IlleNilsson/xmip-core-library-net), the one reading of a URI every technology calls: scheme, authority, path and decoded query. Until 2026-09-28 it was read through the transport capability's `socket::target`, which split it on its first slash and left the query in the path.

A `0x` number in a target is read by `codec::hex::prefixed_number` in [xmip-core-library-codec](https://github.com/IlleNilsson/xmip-core-library-codec), which refuses a sign; until 2026-09-28 it was read with `from_str_radix`, which took `0x+7e8`.

## Acknowledgement

A receive is an ISDU read of the parameter, which consumes nothing at the
device. Its verdict therefore has nothing to tell the device, whichever it is:
a receive cycle that did not complete loses nothing, and the next read finds
the parameter again. The parameter's bytes arrive whole.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
