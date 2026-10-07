//! Throwaway experiments answering the open questions before enoxian adopts
//! Iroh. Each subcommand prints what it measured; nothing here is product code.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::{
    endpoint::{presets, Connection, PortmapperConfig},
    tls::CaTlsConfig,
    Endpoint, EndpointAddr, RelayMap, RelayMode, RelayUrl, SecretKey,
};

const ALPN: &[u8] = b"enoxian-spike/1";

#[tokio::main]
async fn main() -> Result<()> {
    // The embedded relay server builds rustls configs from the process-wide
    // provider; enoxian embedding iroh-relay would need this too.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("keys") => keys(),
        Some("pair") => pair(false).await,
        Some("relay-only") => pair(true).await,
        Some("multi") => multi(args.get(2).and_then(|n| n.parse().ok()).unwrap_or(5)).await,
        Some("relay-down") => relay_down().await,
        Some("all-relays") => all_relays().await,
        Some("listen") => listen(&args[2..]).await,
        Some("dial") => dial(&args[2..]).await,
        _ => {
            eprintln!(
                "usage: iroh-spike keys|pair|relay-only|multi [n]|relay-down|all-relays\n\
                 \x20      iroh-spike listen [--relay-only] [--portmapper] [--for SECS]\n\
                 \x20      iroh-spike dial ID RELAY_URL [--relay-only] [--portmapper] [--secs N] [--mib N]"
            );
            Ok(())
        }
    }
}

// ── 1. Identity ──────────────────────────────────────────────────────────────

/// The Circle key enoxian derives today is a raw 32-byte Ed25519 seed. Does
/// Iroh accept the same seed, give the same public key, and can the old
/// libp2p PeerId be recomputed from the authenticated remote id?
fn keys() -> Result<()> {
    for i in 0..1000u32 {
        let mut seed = [0u8; 32];
        seed[..4].copy_from_slice(&i.to_be_bytes());
        seed[4] = 0xa5;

        let lp_secret = libp2p_identity::ed25519::SecretKey::try_from_bytes(seed)?;
        let lp_pair = libp2p_identity::ed25519::Keypair::from(lp_secret);
        let lp_public = libp2p_identity::PublicKey::from(lp_pair.public());
        let peer_id = lp_public.to_peer_id();

        let iroh_secret = SecretKey::from_bytes(&seed);
        let endpoint_id = iroh_secret.public();

        anyhow::ensure!(
            endpoint_id.as_bytes() == &lp_pair.public().to_bytes(),
            "seed {i}: public keys differ"
        );
        let recovered = libp2p_identity::PublicKey::from(
            libp2p_identity::ed25519::PublicKey::try_from_bytes(endpoint_id.as_bytes())?,
        )
        .to_peer_id();
        anyhow::ensure!(recovered == peer_id, "seed {i}: PeerId differs");
        if i == 0 {
            println!("seed 0: PeerId {peer_id}\n        EndpointId {endpoint_id}");
        }
    }
    println!("keys: 1000 seeds — same public key, PeerId recovered from EndpointId every time");
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn endpoint(relays: &RelayMap, relay_only: bool) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Custom(relays.clone()))
        .ca_tls_config(CaTlsConfig::insecure_skip_verify())
        .portmapper_config(PortmapperConfig::Disabled);
    if relay_only {
        builder = builder.clear_ip_transports();
    }
    Ok(builder.bind().await?)
}

/// Echo every bi stream back; keep connections until the endpoint closes.
fn serve(endpoint: Endpoint) {
    tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            tokio::spawn(async move {
                let Ok(conn) = incoming.await else { return };
                while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                    tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut recv, &mut send).await;
                        let _ = send.finish();
                    });
                }
            });
        }
    });
}

async fn echo(conn: &Connection, bytes: usize) -> Result<Duration> {
    let start = Instant::now();
    let (mut send, mut recv) = conn.open_bi().await?;
    let payload = vec![7u8; bytes];
    let writer = async {
        send.write_all(&payload).await?;
        send.finish()?;
        anyhow::Ok(())
    };
    let (written, back) = tokio::join!(writer, recv.read_to_end(bytes + 1));
    written?;
    anyhow::ensure!(back?.len() == bytes, "short echo");
    Ok(start.elapsed())
}

fn describe_paths(conn: &Connection) -> String {
    let paths = conn.paths();
    let mut out: Vec<String> = paths
        .iter()
        .map(|p| {
            format!(
                "{}{} {:?}",
                if p.is_selected() { "*" } else { " " },
                if p.is_relay() { "relay" } else { "ip" },
                p.remote_addr()
            )
        })
        .collect();
    out.sort();
    out.join(" | ")
}

fn selected_is_relay(conn: &Connection) -> Option<bool> {
    conn.paths()
        .iter()
        .find(|p| p.is_selected())
        .map(|p| p.is_relay())
}

// ── 2/4. Pair: relay-URL-only dialing, path selection on one machine ────────

/// Two endpoints on this Mac. B dials A knowing only A's id and relay URL —
/// how enoxian members would address each other. Watch which path is chosen.
async fn pair(relay_only: bool) -> Result<()> {
    let (relays, relay_url, _relay) = iroh::test_utils::run_relay_server().await?;
    let a = endpoint(&relays, relay_only).await?;
    let b = endpoint(&relays, relay_only).await?;
    serve(a.clone());
    tokio::time::timeout(Duration::from_secs(10), a.online())
        .await
        .context("A never got online with the relay")?;

    let addr = EndpointAddr::new(a.id()).with_relay_url(relay_url.clone());
    let start = Instant::now();
    let conn = b.connect(addr, ALPN).await?;
    println!(
        "{}: connected in {:?}, remote_id matches: {}",
        if relay_only { "relay-only" } else { "pair" },
        start.elapsed(),
        conn.remote_id() == a.id()
    );
    println!("  t=0     paths: {}", describe_paths(&conn));
    let rtt = echo(&conn, 1024).await?;
    println!("  first 1 KiB echo: {rtt:?}");

    let mut switched = None;
    for tick in 1..=20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if switched.is_none() && selected_is_relay(&conn) == Some(false) {
            switched = Some(tick as f32 * 0.5);
        }
    }
    println!("  t=10s   paths: {}", describe_paths(&conn));
    match switched {
        Some(s) => println!("  selected path moved off the relay after ~{s}s"),
        None => println!("  selected path stayed on the relay for 10s"),
    }
    let bulk = echo(&conn, 8 * 1024 * 1024).await?;
    println!(
        "  8 MiB echo: {bulk:?} (~{:.0} MiB/s round trip)",
        16.0 / bulk.as_secs_f64()
    );
    if relay_only {
        let any_ip = conn.paths().iter().any(|p| p.is_ip());
        println!("  any direct path ever opened: {any_ip}");
    }
    conn.close(0u32.into(), b"done");
    b.close().await;
    a.close().await;
    Ok(())
}

// ── 3. Many endpoints in one process ─────────────────────────────────────────

/// enoxian needs one endpoint per Circle (each Circle has its own key).
/// What does each extra endpoint cost while idle and connected?
async fn multi(n: usize) -> Result<()> {
    let pid = std::process::id();
    let (relays, relay_url, _relay) = iroh::test_utils::run_relay_server().await?;
    let base = snapshot(pid);
    println!("multi: baseline (relay running)  {base}");

    let mut endpoints = Vec::new();
    for _ in 0..n {
        let ep = endpoint(&relays, false).await?;
        serve(ep.clone());
        endpoints.push(ep);
    }
    for ep in &endpoints {
        tokio::time::timeout(Duration::from_secs(10), ep.online()).await?;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    println!("multi: {n} endpoints, online    {}", snapshot(pid));
    println!(
        "       sockets per endpoint: {:?}",
        endpoints[0].bound_sockets()
    );

    // Each endpoint connects to the next one, as members of n Circles would.
    let mut conns = Vec::new();
    for i in 0..n {
        let target = &endpoints[(i + 1) % n];
        let addr = EndpointAddr::new(target.id()).with_relay_url(relay_url.clone());
        conns.push(endpoints[i].connect(addr, ALPN).await?);
    }
    for conn in &conns {
        echo(conn, 1024).await?;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    println!("multi: + {n} connections         {}", snapshot(pid));

    let cpu_before = cpu_seconds(pid);
    tokio::time::sleep(Duration::from_secs(30)).await;
    let idle = cpu_seconds(pid) - cpu_before;
    println!(
        "multi: idle 30s with {n} endpoints: {:.2}s CPU ({:.1}% of one core)",
        idle,
        idle / 30.0 * 100.0
    );
    for ep in &endpoints {
        ep.close().await;
    }
    Ok(())
}

fn snapshot(pid: u32) -> String {
    let rss = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let lsof = std::process::Command::new("lsof")
        .args(["-n", "-P", "-p", &pid.to_string()])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let udp = lsof.lines().filter(|l| l.contains(" UDP ")).count();
    let tcp = lsof.lines().filter(|l| l.contains(" TCP ")).count();
    let threads = std::process::Command::new("ps")
        .args(["-M", "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count().saturating_sub(1))
        .unwrap_or(0);
    format!(
        "RSS {:>6.1} MiB, UDP sockets {udp:>3}, TCP sockets {tcp:>3}, threads {threads}",
        rss as f64 / 1024.0
    )
}

fn cpu_seconds(pid: u32) -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    // [[hh:]mm:]ss.cc
    out.split(':')
        .fold(0.0, |acc, part| acc * 60.0 + part.parse::<f64>().unwrap_or(0.0))
}

// ── 5. Relay goes away ───────────────────────────────────────────────────────

/// Two relays, as a Circle configured with a fallback would have. Connect,
/// then kill the relay both endpoints use as home.
async fn relay_down() -> Result<()> {
    let (map1, url1, relay1) = iroh::test_utils::run_relay_server().await?;
    let (map2, url2, _relay2) = iroh::test_utils::run_relay_server().await?;
    let relays = map1.clone();
    relays.extend(&map2);
    let a = endpoint(&relays, false).await?;
    let b = endpoint(&relays, false).await?;
    serve(a.clone());
    a.online().await;
    b.online().await;
    let home = |ep: &Endpoint| -> Option<RelayUrl> { ep.addr().relay_urls().next().cloned() };
    println!("relay-down: relays {url1} and {url2}");
    println!("  A home {:?}, B home {:?}", home(&a), home(&b));

    // Direct connection first: does it survive the relay vanishing?
    let addr = EndpointAddr::new(a.id()).with_relay_url(home(&a).context("A has no home")?);
    let conn = b.connect(addr, ALPN).await?;
    echo(&conn, 1024).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    println!("  before: {}", describe_paths(&conn));

    let killed = home(&a) == Some(url1.clone());
    drop(if killed { Some(relay1) } else { None });
    let dead = if killed { &url1 } else { &url2 };
    if !killed {
        println!("  (A's home is relay 2, which this run cannot kill; rerun)");
        return Ok(());
    }
    let down_at = Instant::now();
    println!("  killed relay {dead}");

    match tokio::time::timeout(Duration::from_secs(5), echo(&conn, 1024)).await {
        Ok(Ok(t)) => println!("  existing connection still works after relay loss: echo {t:?}"),
        other => println!("  existing connection broke: {other:?}"),
    }

    // A new peer dialing A by its now-dead home relay URL: how long until a
    // connection succeeds, as A re-homes onto the surviving relay?
    let c = endpoint(&relays, false).await?;
    c.online().await;
    let mut first_ok = None;
    for attempt in 0..60 {
        // Every relay the Circle uses, as a dialer would list them.
        let target = EndpointAddr::new(a.id())
            .with_relay_url(url1.clone())
            .with_relay_url(url2.clone());
        let attempt_start = Instant::now();
        match tokio::time::timeout(Duration::from_secs(5), c.connect(target, ALPN)).await {
            Ok(Ok(conn)) => {
                first_ok = Some(down_at.elapsed());
                println!(
                    "  new dial succeeded on attempt {} after {:?} (A home now {:?}, path {})",
                    attempt + 1,
                    down_at.elapsed(),
                    home(&a),
                    describe_paths(&conn)
                );
                break;
            }
            _ => {
                let spent = attempt_start.elapsed();
                if spent < Duration::from_secs(1) {
                    tokio::time::sleep(Duration::from_secs(1) - spent).await;
                }
            }
        }
    }
    if first_ok.is_none() {
        println!("  no new dial succeeded within ~60 attempts");
    }
    // The same, dialing with A's direct addresses (what a LAN peer would know).
    let direct = a.addr();
    match tokio::time::timeout(Duration::from_secs(5), c.connect(direct.clone(), ALPN)).await {
        Ok(Ok(_)) => println!("  dialing with A's full current address works"),
        other => println!("  dialing with A's full address failed: {other:?}"),
    }
    Ok(())
}

// ── Addressing with every Circle relay ───────────────────────────────────────

/// Members of one Circle pick different home relays. If a dialer lists every
/// relay the Circle uses, does Iroh find the target on whichever it is home on?
/// Relay-only, so success can only come through the target's home relay.
async fn all_relays() -> Result<()> {
    let (map1, url1, _r1) = iroh::test_utils::run_relay_server().await?;
    let (map2, url2, _r2) = iroh::test_utils::run_relay_server().await?;
    let relays = map1.clone();
    relays.extend(&map2);
    let mut ok = 0;
    let mut homes = std::collections::BTreeMap::new();
    for _ in 0..10 {
        let a = endpoint(&relays, true).await?;
        let b = endpoint(&relays, true).await?;
        serve(a.clone());
        a.online().await;
        let home = a.addr().relay_urls().next().cloned();
        *homes.entry(format!("{home:?}")).or_insert(0) += 1;
        // Deliberately list the relays in a fixed order, so half the time the
        // first one is not A's home.
        let addr = EndpointAddr::new(a.id())
            .with_relay_url(url1.clone())
            .with_relay_url(url2.clone());
        let start = Instant::now();
        match tokio::time::timeout(Duration::from_secs(10), b.connect(addr, ALPN)).await {
            Ok(Ok(conn)) => {
                echo(&conn, 1024).await?;
                ok += 1;
                println!("  connected in {:?} via {}", start.elapsed(), describe_paths(&conn));
            }
            other => println!("  failed after {:?}: {other:?}", start.elapsed()),
        }
        b.close().await;
        a.close().await;
    }
    println!("all-relays: {ok}/10 connected; A's home relays: {homes:?}");
    Ok(())
}

// ── Across machines: listen / dial over n0's public relays ──────────────────

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn value<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// n0's production relays, no DNS publishing: peers are dialed by id + relay URL.
async fn public_endpoint(args: &[String], alpns: Vec<Vec<u8>>) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .alpns(alpns)
        .relay_mode(RelayMode::Default);
    if !flag(args, "--portmapper") {
        builder = builder.portmapper_config(PortmapperConfig::Disabled);
    }
    if flag(args, "--relay-only") {
        builder = builder.clear_ip_transports();
    }
    Ok(builder.bind().await?)
}

fn host() -> String {
    std::process::Command::new("hostname")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Print how to reach this endpoint, then echo whatever arrives and report
/// each connection's paths as they change.
async fn listen(args: &[String]) -> Result<()> {
    let ep = public_endpoint(args, vec![ALPN.to_vec()]).await?;
    tokio::time::timeout(Duration::from_secs(20), ep.online())
        .await
        .context("no relay reachable within 20s")?;
    let relay = ep.addr().relay_urls().next().cloned().context("no home relay")?;
    println!("listen on {} ({} {})", host(), std::env::consts::OS, std::env::consts::ARCH);
    println!("ID {}", ep.id());
    println!("RELAY {relay}");
    println!("DIRECT {:?}", ep.addr().ip_addrs().collect::<Vec<_>>());
    let accept = ep.clone();
    tokio::spawn(async move {
        while let Some(incoming) = accept.accept().await {
            tokio::spawn(async move {
                let Ok(conn) = incoming.await else { return };
                println!("accepted {} at {:?}", conn.remote_id(), Instant::now());
                let watch = conn.clone();
                tokio::spawn(async move {
                    let mut last = String::new();
                    while watch.close_reason().is_none() {
                        let now = describe_paths(&watch);
                        if now != last {
                            println!("  listener paths: {now}");
                            last = now;
                        }
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                });
                while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                    tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut recv, &mut send).await;
                        let _ = send.finish();
                    });
                }
            });
        }
    });
    tokio::time::sleep(Duration::from_secs(value(args, "--for", 600u64))).await;
    ep.close().await;
    Ok(())
}

/// Connect to a listener, watch which path carries the connection, then
/// measure throughput on whatever path ended up selected.
async fn dial(args: &[String]) -> Result<()> {
    let id: iroh::EndpointId = args.first().context("missing ID")?.parse()?;
    let relay: RelayUrl = args.get(1).context("missing RELAY_URL")?.parse()?;
    let secs: u64 = value(args, "--secs", 30);
    let mib: usize = value(args, "--mib", 32);
    let ep = public_endpoint(args, vec![]).await?;
    tokio::time::timeout(Duration::from_secs(20), ep.online())
        .await
        .context("no relay reachable within 20s")?;
    println!("dial from {} ({} {})", host(), std::env::consts::OS, std::env::consts::ARCH);

    let start = Instant::now();
    let conn = ep
        .connect(EndpointAddr::new(id).with_relay_url(relay), ALPN)
        .await?;
    let connected = start.elapsed();
    println!("connected in {connected:?}, remote_id matches: {}", conn.remote_id() == id);
    println!("first echo 1 KiB: {:?}", echo(&conn, 1024).await?);

    let mut last = String::new();
    let mut first_direct = None;
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let now = describe_paths(&conn);
        if now != last {
            println!("  t={:>5.1}s paths: {now}", start.elapsed().as_secs_f64());
            last = now;
        }
        if first_direct.is_none() && selected_is_relay(&conn) == Some(false) {
            first_direct = Some(start.elapsed());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let selected = conn
        .paths()
        .iter()
        .find(|p| p.is_selected())
        .map(|p| format!("{:?} rtt {:?}", p.remote_addr(), p.rtt()));
    let bulk = echo(&conn, mib * 1024 * 1024).await?;
    println!(
        "SUMMARY connect={connected:?} direct_after={first_direct:?} selected={selected:?} \
         echo_{mib}MiB={bulk:?} (~{:.1} MiB/s each way)",
        mib as f64 / bulk.as_secs_f64()
    );
    conn.close(0u32.into(), b"done");
    ep.close().await;
    Ok(())
}
