# Iroh spike

Throwaway experiments for the libp2p → Iroh migration. Not part of enoxian's
build: this is a standalone crate, and the repository has no Cargo workspace.

```bash
cd spikes/iroh
cargo run --release -- keys          # Circle key ↔ EndpointId ↔ PeerId
cargo run --release -- pair          # two endpoints, local relay
cargo run --release -- relay-only
cargo run --release -- multi 5       # cost of one endpoint per Circle
cargo run --release -- relay-down    # home relay dies
cargo run --release -- all-relays    # dialing with every Circle relay

# Across machines, over n0's public relays:
cargo run --release -- listen                    # prints ID and RELAY
cargo run --release -- dial <ID> <RELAY>         # from the other machine
```

Flags: `--relay-only`, `--portmapper` (off by default), `--for SECS` (listen),
`--secs N` and `--mib N` (dial).
