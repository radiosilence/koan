# Sharing your Mac

The macOS app can be a kōan server: phones and other devices sign in to it and play the Mac's library, as they would a server running on another machine. Nothing else needs installing.

## Turning it on

1. In **Settings → Library**, add your music folders.
2. In **Settings → Server**, choose **Shares its library**.
3. Make your account. It is the server's admin: it invites other people and approves devices that sign in with a code.
4. **Link a Device** shows a QR code. Point the phone's camera at it, and kōan opens there signed in.

The foot of the sidebar always says what the Mac is doing: playing its own library, connected to a server, or sharing at an address. Clicking it opens Settings → Server.

A Mac shares or connects, not both. To sign in to another server, turn sharing off first; to share, sign out of the server first.

The Mac serves while kōan is open and the Mac is awake. **Keep running in the menu bar** (Settings → Devices) keeps it serving with the window closed.

## On your network

Phones on the same Wi-Fi reach the Mac at the address Settings shows, such as `http://192.168.1.20:4000`, or by its Bonjour name, such as `http://mac.local:4000`. The port is set under **Port**.

The connection is plain HTTP: sign-ins and music cross your network unencrypted, as with any `http://` server. That is the usual trade-off on a home network. On a network you share with people you don't trust, reach the Mac through Tailscale instead (below).

## Away from home

kōan does not run a relay or a tunnel of its own. These give the Mac an address reachable from anywhere, and the first two encrypt the connection:

- **Tailscale** (recommended). Install Tailscale on the Mac and the phone and sign both in to the same tailnet. The phone reaches the Mac at its tailnet address, such as `http://100.101.102.103:4000`, encrypted end to end, and nothing is published to the internet. To use its MagicDNS name (`mac.tailnet.ts.net`) instead, put that name under **Public address**: the Mac answers only to names it has been told, so a web page elsewhere cannot reach it by pointing a name of its own at the Mac. To reach the Mac from a device without Tailscale, `tailscale funnel 4000` publishes it at `https://<mac>.<tailnet>.ts.net` with a certificate Tailscale provides.
- **Cloudflare Tunnel.** Free, with HTTPS at Cloudflare's edge, but needs a domain on Cloudflare. Cloudflare's terms restrict serving large media through tunnels on the free plan; personal use is common, but it is a risk to weigh.
- **Not recommended:** ngrok's free plan, which transfers 1 GB a month; Cloudflare quick tunnels, whose address changes on every start and signs every phone out; and forwarding a router port to the Mac, which publishes plain HTTP to the internet.

Put the tunnel's HTTPS address under **Public address**. Invites and QR codes then send phones there rather than to the Mac's address on your network.

## What sharing serves

The Subsonic API and kōan's own extensions, so any Subsonic app can sign in as well as kōan's. The web UI and MCP are not served from the Mac; run [the server](headless-server.md) for those.

Sign-in settings under `[graphql]` in `config.toml` (`auth_enabled`, token lifetimes, `allowed_hosts`, `cookie_secure`) apply to the Mac's server as to the terminal UI's. The port and whether sharing is on are `[share]` in `config.local.toml`; see [Configuration](../reference/configuration.md#share).
