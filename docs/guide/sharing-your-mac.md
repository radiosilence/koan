# Sharing your Mac

The macOS app can be a kōan server: phones and other devices sign in to it and play the Mac's library, as they would a server running on another machine. Nothing else needs installing.

## Turning it on

1. In **Settings → Library**, add your music folders.
2. In **Settings → Server**, choose **Shares its library**.
3. Make your account. It is the server's admin: it invites other people and approves devices that sign in with a code. **Reset Password…** beside it sets a new one without the old, and signs its other devices out.
4. **Link a Device** shows a QR code. Point the phone's camera at it, and kōan opens there signed in.

The foot of the sidebar always says what the Mac is doing: playing its own library, connected to a server, or sharing at an address. Clicking it opens Settings → Server.

A Mac shares or connects, not both. To sign in to another server, turn sharing off first; to share, sign out of the server first.

The Mac serves while kōan is open and the Mac is awake. **Keep running in the menu bar** (Settings → Devices) keeps it serving with the window closed.

## On your network

Phones on the same Wi-Fi reach the Mac at the address Settings shows, such as `http://192.168.1.20:4000`, or by its Bonjour name, such as `http://mac.local:4000`. The port is set under **Port**.

The connection is plain HTTP: sign-ins and music cross your network unencrypted, as with any `http://` server. That is the usual trade-off on a home network. On a network you share with people you don't trust, reach the Mac through Tailscale instead (below).

## Away from home

### With Tailscale

With [Tailscale](https://tailscale.com/download) running on the Mac, **Away from home** in Settings shows it as detected, with the Mac's name on your tailnet.

**Invite phones to** decides the address invites and QR codes carry:

- **Your tailnet** (the default with Tailscale): `http://<mac>.<tailnet>.ts.net:4000`. A phone with Tailscale reaches the Mac from anywhere, encrypted by Tailscale, with nothing published to the internet. This is the simplest way to listen away from home: install Tailscale on the phone too.
- **Tailscale Funnel**: publishes the Mac on the internet at `https://<mac>.<tailnet>.ts.net`, with a certificate Tailscale provides, for phones without Tailscale. kōan asks first, publishes it again whenever the Mac starts sharing, and takes it down when sharing stops or another choice is made.
- **This network**: the Mac's address on the local network.
- **Custom address**: a tunnel's HTTPS address (below).

Funnel publishes at the Mac's own name on the tailnet; to change the address, rename the Mac in Tailscale. It uses port 443 unless something else on the Mac is already published there, then 8443 or 10000, and leaves what else Tailscale serves alone. Funnel has to be turned on for the tailnet first, in its access controls; until it is, **Tailscale Funnel** is not offered. Quitting kōan takes its Funnel down, and starting to share again puts it back; a Funnel set up by hand is never touched.

### Without Tailscale

A tunnel gives the Mac an HTTPS address of its own; choose **Custom address** and put it there. Run the tunnel on the Mac itself, pointed at `http://127.0.0.1:4000`: the Mac takes the client address a tunnel passes on only from a tunnel running on it, and one elsewhere on the network would make every visitor look like a device at home.

- **Cloudflare Tunnel.** Free, with HTTPS at Cloudflare's edge, but needs a domain on Cloudflare. Cloudflare's terms restrict serving large media through tunnels on the free plan; personal use is common, but it is a risk to weigh.
- **Not recommended:** ngrok's free plan, which transfers 1 GB a month; Cloudflare quick tunnels, whose address changes on every start and signs every phone out; and forwarding a router port to the Mac, which publishes plain HTTP to the internet.

kōan does not run a relay or a tunnel of its own.

## What sharing serves

The Subsonic API and kōan's own extensions, so any Subsonic app can sign in as well as kōan's, and share pages. The web UI, GraphQL and MCP are not served from the Mac, so no account can drive the Mac's own player; run [the server](headless-server.md) for those.

Sign-in is always on: the Mac refuses to share with `[graphql] auth_enabled = false`. `[graphql] allowed_hosts` adds names it answers to. Its sharing settings are `[share]` in `config.local.toml`, apart from a headless server's on the same Mac; see [Configuration](../reference/configuration.md#share).
