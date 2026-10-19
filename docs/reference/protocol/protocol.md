# Protocol

kōan's apps talk to a kōan server over Subsonic, and to each other through it and directly on the local network. Every kōan client links koan-core and shares its Rust types, but old and new versions meet across a release, and other clients and servers may speak the same protocol. These pages are its specification.

| Page | Covers |
|------|--------|
| [Link](link-protocol.md) | The standing WebSocket between an app and its server: every message, relaying between devices, acks, shares |
| [Nearby](nearby-protocol.md) | Devices on one network finding and controlling each other without the server: discovery, the handshake, trust |
| [Pairing and invites](pairing-and-invites.md) | Signing a device in without typing the password on it |
| [Server extensions](server-extensions.md) | Each `koan*` OpenSubsonic extension: endpoints, parameters, answers, credentials |

## Schema

[`docs/reference/protocol/schema/`](https://github.com/radiosilence/koan/tree/main/docs/reference/protocol/schema) holds JSON Schema (draft 2020-12) for every message: `link-command.json` and `link-report.json` for the link and nearby connections, `envelope.json` for a command carrying an ack id, `pair-message.json` for a pairing socket, and `nearby-frame.json` for the nearby handshake and signed frames. They are generated from the Rust types, so they state what the code accepts; these pages state what the messages mean. `just protocol-schema` regenerates them, and the test suite fails when the committed files differ from what the types produce.

## Capabilities

A client decides what to use from what the other end says it supports, never from the server calling itself kōan.

- **Server extensions.** A kōan server lists its additions in `getOpenSubsonicExtensions`, probed once per sign-in. A client uses an extension only where it is listed. Every extension is at version 1, and clients check the name only. One exception remains for servers that linked before `koanLink` was listed: a server whose OpenSubsonic `type` is `koan` is linked to anyway.
- **Link parameters.** A client says what it understands when it opens the link: `devices=1`, `acks=1`, `deviceKey`. The server sends `devices`, `shares` and `deviceKeys` only to links that asked, and attaches an ack id only for a device that sent `acks=1`.
- **Device flags.** Each device in `devices` carries `acks`, so a sender knows whether to wait for an answer from it. On the local network the listener's `hello` says the same with `acks`, and with a `nonce` that it can prove itself; a `reports.` prefix on the nonce says it signs its reports.

## Compatibility rules

These hold today, and a change to the protocol keeps them.

**Unknown messages are ignored.** A receiver that cannot parse a frame, or does not know its `type`, drops it and keeps the connection open. Only a close, an I/O error or a missed ping ends a link or a nearby connection. A command that is parsed and then refused is answered with a `refused` ack when it carried an id.

**Unknown fields are ignored.** No message type rejects fields it does not know.

**Optional fields may be missing.** A missing optional field takes its default: `false`, `0`, empty, or absent. A missing required field, or an unknown value of a nested enum (`repeat` mode, `sleepTimer` kind, output kind, ack `result`), fails the whole message, which is then ignored. Required fields are those the pages list without saying they may be absent.

**Relayed commands are re-serialised.** The server parses every command it relays, keeps for an absent device, or pushes, and sends its own serialisation. A field the server does not know does not survive the hop, and a command type it does not know is not relayed. A new field on a relayed command therefore needs the server updated before devices can rely on it, and anything that must survive an older server, as the ack id does, travels beside the command rather than inside it.

**A new capability is announced before it is used.** A new endpoint or behaviour on the server comes with a new extension name. A new message a device must understand is sent only to links that asked for it with a link parameter, or to devices whose `devices` entry or `hello` says they understand it. News an older client can safely miss, such as `historyChanged`, may go to every link, since it is ignored there.

**Old meets new.** An older client of a newer server sees the extensions it knows and ignores the rest; messages it was not built for are dropped. A newer client of an older server finds extensions missing from the list and leaves those features off, and commands for devices are carried with the fields the older server knows.
