# Pairing and invites

Two ways a device signs in to a kōan server without anyone typing the account's password on it. Both end with the device holding an API key of its own, which it signs every later request with and which can be revoked alone. See [Authentication](../../guide/authentication.md) for how they fit with the other sign-ins.

## Invites (`koanInvite`)

An admin makes an invite for an account with `/rest/koanInvite?username=…` (or `koanCreateUser`, which makes the account too). The answer carries a token: a JWT signed with the server's key whose payload claims are `typ` (`koan-invite`), the account (`sub`, its id, and `username`), a mark of its password hash (`pwd`), when it was made (`iat`) and an expiry seven days on (`exp`). Nothing is stored when one is made, so a link signs in as many devices as its holder has until it expires or the account's password changes, which changes the mark. Resetting the password (`reset=true`) is how a link sent to the wrong place is withdrawn.

### Link format

```
https://koan.rocks/join/#server=<server>&username=<username>&invite=<token>
koan://join?server=<server>&username=<username>&invite=<token>
```

The parameters are form-encoded. On koan.rocks they ride in the fragment, which browsers do not send, so the site never sees them; the page opens the app through its universal link, or offers the downloads and the `koan://` link. `/join` without the trailing slash parses too. `server` must be an `http(s)` address. A link missing `server`, `username` or `invite` is not an invite; parameters it does not know, such as the `password` older links carried, are ignored.

A server address with the account in it, `https://user:password@host`, is read as a sign-in with that password rather than an invite.

### Redeeming

`/rest/koanJoin?invite=<token>&name=<device name>` takes no other credential: the token is it. The server checks its signature, expiry and password mark, and answers `join` with `username` and `apiKey`.

## Pairing (`koanPair`)

For a device with no keyboard, such as a television, that knows only the server's address.

### The device

The device opens a WebSocket at `/rest/koanPair?name=<device name>` with no credentials. The server refuses one carrying an `Origin` header, which every browser sends and no app does, so a web page on the network cannot open a pairing and read the key it is sent. Opening is rate-limited per address, with at most three pairings waiting per address (or IPv6 /64) and 256 in all.

The server sends JSON text frames tagged by `type`:

| Message | Fields | When |
|---------|--------|------|
| `pending` | `id` (unguessable, for the link), `code` (`XXXX-XXXX`, Crockford base32: no I, L, O or U), `expiresIn` (seconds, 600) | Once, on opening |
| `approved` | `username`, `apiKey` | Someone approved it; the socket then closes |
| `declined` | | Someone turned it away |
| `expired` | | Ten minutes passed |

The server pings every 30 seconds; a device that hears nothing for 90 takes the connection as gone. A pairing lives in memory only, so a server restart loses nothing the device cannot ask for again.

### The link

```
https://koan.rocks/pair/#s=<server>&p=<id>
koan://pair#s=<server>&p=<id>
```

The device shows the code and this link, usually as a QR code. As with invites the parameters ride in the fragment; `koan://pair` also accepts them as a query. `s` must be an `http(s)` address.

### The approver

Someone signed in to the same server, on a phone, a Mac or the web UI's `/pair` page:

1. `/rest/koanPairInfo?pair=<id or code>` answers `pair` with `device` (the name the device gave), `from` (the address it asked from) and `local` (whether that address is on a private network), for the app to show before asking.
2. `/rest/koanPairApprove?pair=<id or code>` makes an API key on the approver's account and sends it down the waiting socket. The answer is the same `pair` element. With `decline=true` the device is turned away instead.

Approving needs the account's own password or an API key, since it mints a key; an app password or the shared secret is refused. Looking a pairing up or declining it takes any credential. A device is signed in with no more than the approver's account may do. Approving does not go through an invite: an invite token signs in any number of devices for a week, where an approval hands one device one key of its own.

### RFC 8628

Pairing has the shape of the OAuth device authorization grant. The `code` is its user code, the link its `verification_uri_complete` (carrying `id` in place of the code, which makes the link a second, unguessable user code), the open socket stands in for the device code, and `approved`, `declined` and `expired` are its token response and its `access_denied` and `expired_token` errors. A pairing belongs to no account until it is approved, which leaves any authenticated caller able to look up a pending device's name and address, and to decline it. A code has 40 bits, but a guess through `koanPairInfo` or `koanPairApprove` is matched against every pending pairing, up to 256 of them, so a hit takes around 2^32 requests; failed lookups are not rate limited, which §5.1 recommends.

The protocol itself is not RFC 8628, for three reasons. The device needs an OpenSubsonic API key, which is how the apps sign in, rather than an OAuth token. kōan's OAuth endpoints are served only with `sharing.public_url` set, and pairing has to work on a server without it, such as one reachable only on the local network. And the RFC only polls, where a socket delivers the outcome the moment it is settled. The server revokes an approved key only when it already knows the device's socket has closed; a key sent down a socket that died without the server noticing, before a keepalive ping fails, stays on the account.
