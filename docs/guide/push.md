# Push and the relay

iOS suspends kōan shortly after it leaves the screen, unless it is playing, and its connection to the server goes with it. A push from Apple is the only supported way back. kōan's servers send these pushes through a relay at `push.koan.rocks`, run by kōan's developer. This page explains why the relay exists, what it can see, and how to turn it off.

## Why a relay

Apple delivers pushes to an app only when they are signed with a key belonging to the developer who ships it. A self-hosted server cannot hold that key: anyone holding it can send any kōan user's phone anything, and the developer's Apple account answers for every push sent under it. Without the relay, only a server run by the developer could wake a phone, and every self-hosted server would lose its phones as soon as iOS suspended them.

The relay holds the key instead and forwards pushes for any kōan server, including the developer's own. It is designed so that holding the key does not mean trusting the servers that use it:

- A server never sees a device token. It holds a **handle**, which only the relay can open.
- The relay writes everything Apple delivers. A server chooses only the kind of push, so it cannot put its own text on a phone.
- Quotas bound how often any one server, and any one device, can be pushed to.

## How it works

![The app registers its token with the relay and gives its server the handle; the server asks the relay to push, and the relay sends Apple a push it wrote itself](../../site/public/diagrams/push-relay.svg)

1. When kōan signs in to a server that offers push, it asks Apple for a device token and registers that token with the relay. On a TestFlight or App Store build the registration carries an [App Attest](https://developer.apple.com/documentation/devicecheck/establishing-your-app-s-integrity) assertion, Apple's confirmation that it comes from kōan's own app on a real device; the relay refuses any registration without one.
2. The relay returns a handle, the token sealed under a key only the relay holds, and a revoker, which only the app keeps. The app gives the handle to its server over the link. Each server gets a handle of its own.
3. When the server needs the phone and it is not linked, it sends the relay the handle and the kind of push: a silent wake, or a notification.
4. The relay opens the handle, checks its quotas, and sends Apple a push it wrote itself.

A silent wake gives the app about half a minute in the background. It links, takes whatever was waiting for it (a sync, a queue change, a command) and is listed as awake, so music sent to it from then on plays straight away. Choosing a sleeping phone in Play on sends this kind of wake. A request to play that reaches a phone still suspended becomes a notification instead, reading "Something is waiting to play. Tap to open." Tapping it opens kōan, which hands the notification's claim back to its server over the link, and the server plays what it was kept for. The claim is a random value meaningful only to the server that issued it, and only for ten minutes.

An Apple TV takes silent wakes only, since tvOS shows no notifications. Live Activities on the phone are updated through the relay in the same way.

Handles last 60 days, and the app registers again halfway through. Signing out of a server revokes every handle the device was given, so a server the phone has left cannot push to it again, whether or not it has forgotten the handle.

## What the relay sees and keeps

For each request the relay sees the caller's IP address, the handle, which it opens to a device token in memory, the kind of push and, for a notification, its claim. During registration and revocation it also sees the device token directly.

It does not see what is playing, track names, account names, or which server a handle belongs to. It writes no request log, and nothing it writes to disk or to its own log contains a token, a handle, an address or a server.

It keeps:

- a keyed hash of each device token that has been revoked or that Apple reports as gone, with the time, for up to 365 days, so that an old handle for that device cannot be used again;
- quota counters in memory, keyed by hashes of tokens and by caller addresses, each dropped once its allowance has refilled;
- totals of requests by route and outcome, with nothing identifying.

Apple, as with every push service on iOS, sees the device token and the relay's push. The [protocol page](../reference/protocol/push-relay.md) specifies the relay's API, handles, quotas and revocation in full, and the relay's own source is at [radiosilence/koan-push-relay](https://github.com/radiosilence/koan-push-relay).

## Turning it off

Push is on by default. To keep a server off the relay entirely:

```toml
# config.toml
[push]
enabled = false
```

or `KOAN_PUSH__ENABLED=false` in the environment, or `push: { enabled: false }` in the Pulumi package. With push off, the server does not list the `koanPush` extension, so apps signed in to it never ask for permission to show notifications and never register with the relay on its behalf. Phones are then reachable only while kōan is open or playing: anything sent to a suspended phone is applied the next time the app is opened, and a sleeping phone is listed in Play on but cannot be woken.

The relay's address is not configurable. The app registers with kōan's relay, the only one holding the key Apple requires, so a server has to push to the same relay.

## Requirements and troubleshooting

The server needs outbound HTTPS to `push.koan.rocks`. Nothing else needs configuring, and nothing needs opening to the internet: the relay never connects to your server.

The phone needs a version of kōan from 0.67 onwards. Earlier versions sent their device token to the server directly, which servers no longer accept, so a phone running one cannot be woken until it is updated and opened once.

Each step is logged on the server: `push: through https://push.koan.rocks` at startup, `wake: <device> has given no push handle` when a phone has not registered yet, and `wake: wake push for <device> answered by the relay in 140ms` followed by `wake: <device> linked 1027ms after its wake push` when a wake works. [Waking a device](devices.md#waking-a-device) describes the other ways a device is woken, and what each log line means.
