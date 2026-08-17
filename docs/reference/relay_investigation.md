# Enoxian Public Relay & Rendezvous Investigation

## Overview
An attempt was made to deploy a public bootstrap server on DigitalOcean to act as a fallback discovery (Rendezvous) and tunneling (Circuit Relay) server. The goal was to allow two Enoxian devices behind strict NATs to sync seamlessly over the public internet without requiring a VPN like Tailscale.

While the first server deployed successfully, the clients failed to establish a P2P sync stream through it due to several interconnected architectural limitations in how Enoxian wrapped `libp2p` transports and reused ports.

> Current implementation note: the bootstrap service is now expected to expose
> rendezvous over QUIC/UDP on `--port` and circuit relay over TCP on
> `--relay-port` (default `--port + 1`). The HTTP `/peer-id` endpoint remains on
> TCP `--port` for CLI auto-resolution.

## Core Issues Discovered

### 1. DNS Resolution in Transports
At the time, the Enoxian libp2p swarm was built without the `libp2p::dns` wrapper. When an invite contained a domain name (e.g., `/dns4/relay.enoxian.com/udp/36521/quic-v1/...`), the client crashed with `Multiaddr is not supported`.
- **Fix Applied:** Wrapped the TCP, Relay, and QUIC transports in `libp2p::dns::tokio::Transport::system(...)` in `src/lifecycle.rs`.

### 2. Rendezvous Registration Requires External IPs
The `libp2p::rendezvous` client is strict: it will not send a registration to the server if the local node has an empty `external_addrs` list.
- When behind a NAT without Tailscale, a device only has private LAN IPs (`192.168.x.x`, `10.x.x.x`). 
- Without an external IP or an advertised relay circuit, the device silently connects to the Rendezvous server but refuses to register itself.
- **Result:** When the second device connects, the server responds with `served 0 registrations`, and discovery fails.

### 3. Circuit Relay Addresses Must Be Explicitly Advertised
Even when a device successfully connects to the Relay server and secures a reservation, it must explicitly synthesize a `/p2p-circuit` multiaddr and add it to its own `external_addrs` list.
- **Fix Attempted:** Added an event handler for `RelayClient(ReservationReqAccepted)` to manually push the `/p2p-circuit` address to `swarm.add_external_address()` and force an immediate rendezvous re-registration.

### 4. Transport Protocol Mismatch (The Blocker)
The `libp2p::relay` protocol fundamentally operates over **TCP**. It does not support tunneling over a connectionless QUIC/UDP stream.
- When configured to use TCP (`/tcp/36521/p2p/...`), older deployments collided with the HTTP `/peer-id` listener on the same TCP port.
- Earlier client builds also wrapped relay TCP in the circle `pnet` transport.
- The public DigitalOcean bootstrap server explicitly does *not* hold the circle PSK.
- **Result:** depending on build/deployment version, the public server either received HTTP-incompatible libp2p relay traffic or `pnet` handshake bytes it could not decrypt, causing `Handshake failed: unexpected end of file`.

## Conclusion
The intended fix is to keep the public relay outside the circle PSK boundary:
direct TCP remains PSK-protected, while relay TCP is Noise/Yamux without `pnet`
and listens on a separate TCP port from HTTP.

With the split transport design, a public non-circle relay can be used without
holding the circle PSK. Direct member-to-member TCP remains PSK-protected, while
public relay and rendezvous connections use Noise/Yamux without `pnet`. A device
behind strict NAT still needs a successful relay reservation before rendezvous
registration can advertise a reachable address.

## Next Steps / Required Architecture Changes
To support public relay servers, the SwarmBuilder must keep the transports separated:
1. **Direct TCP Transport:** Wrapped in `pnet` and `noise` (for LAN and VPN direct dialing).
2. **Relay TCP Transport:** Wrapped in `noise` but **excluding** `pnet`, allowing it to communicate with public infrastructure.
3. **Rendezvous QUIC Transport:** Wrapped in `noise` but excluding `pnet`, allowing registration with public infrastructure.
4. The deployment must not run HTTP `/peer-id` and libp2p relay on the same TCP port.
5. The application logic must ensure that data sent over the relay circuit remains encrypted (Noise provides end-to-end encryption, but the lack of `pnet` means the outer transport layer is exposed to the relay).

If both devices still fail after deploying the split-port bootstrap, inspect
`enox status --json` for a non-empty `relay_addrs`, an accepted relay
reservation in daemon logs, and rendezvous registrations served by the VPS.
