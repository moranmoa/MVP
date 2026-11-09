# Helper servers

These containers never store or read messages. They only help computers find
each other and pass along encrypted traffic when a direct connection fails.

| Service | What it does | Needed from |
| --- | --- | --- |
| `relay` | Bootstrap (DHT entry point) and circuit relay | Closed alpha |
| `caddy` | `wss://` with automatic TLS in front of the relay | When browsers join (`--profile proxy`) |
| `seeder` | Placeholder for the update seeder; exits with "not built yet" | With the updater, B8 (`--profile seeder`) |
| `coturn` | TURN relay for voice and the Web client | Phase 2 (`--profile voice`) |

The bootstrap node and the relay are one service: the relay's DHT is what
clients bootstrap from, and a second container on the same PC would add
nothing. A second bootstrap node at another provider comes before the open
alpha (GAP-9).

The seeder stays a placeholder until B8 builds it. A headless sync node is
deliberately not run as a seeder: until sync pulls are gated on membership
(GAP-4) it would hand every server's log to anyone who asks.

## Run on this PC

1. Docker Desktop must be running (it needs WSL 2, see below).
2. `copy .env.example .env` and fill in your domain. `.env` is git-ignored.
3. From this folder:

   ```
   docker compose up -d --build
   docker compose ps          # relay should say "(healthy)"
   docker compose logs relay
   ```

   The log line `relay node started peer_id=...` shows the node's ID. Clients
   use `/dns4/<domain>/tcp/4001/p2p/<peer_id>` as the bootstrap address. The
   ID is stable: the key lives in the `relay-data` volume, not in git (see
   [Backing up the relay key](#backing-up-the-relay-key)).

## How the containers are locked down

All of it is in `docker-compose.yml`:

- `restart: unless-stopped`: comes back after a crash or a reboot of Docker.
- Healthcheck: `relay-node healthcheck` connects to the relay's own TCP
  port every 30 s. `docker compose ps` shows `healthy`/`unhealthy`, and Caddy
  waits for `healthy` before starting. Plain Docker does not restart an
  `unhealthy` container on its own; a process that exits is restarted.
- Read-only root filesystem: the relay can only write its key volume
  (`/data`) and a small `/tmp`. Caddy can only write its certificate volumes.
- Every Linux capability is dropped (Caddy keeps only the one for binding
  80/443), and `no-new-privileges` is set.
- Limits: relay 1 CPU / 256 MB / 256 processes, Caddy 0.5 CPU / 128 MB.
  Raise the relay's `memory` if `docker stats` shows it near the limit once
  many testers connect.
- Logs: at most 5 files of 10 MB per container (`json-file` rotation).
- coturn has the restart, log and limit settings but not the read-only root
  yet: that is untested until voice is switched on.

And in the relay itself (`relay/src/main.rs`):

- Connection limits: at most `MAX_CONNECTIONS` (default 512) established
  connections, 8 per peer and 64 half-open incoming ones. New connections
  are also refused once the process uses `MAX_MEMORY_MB` (default 200, below
  the container's 256 MB). Raise both together with the compose `memory`.
- The DHT runs on `/mvs/kad/1.0.0`, not the IPFS default, so it can't merge
  with the public IPFS DHT. Only public addresses that peers announce go into
  it: private, loopback, link-local and CGNAT addresses are dropped, so a
  client's home network layout is never handed out. For a test harness or
  a LAN-only setup, `ALLOW_PRIVATE_ADDRS=1` turns this filter off (never on
  the public relay). Until `core/sync` switches, the relay also answers on
  `/ipfs/kad/1.0.0` so current clients keep working (TODO: drop it then).
- `node.key` is created readable by its owner only (0600), and the relay
  **refuses to start** if the file is readable by group or others. Keys
  created before this check are 0644. Fix once, with the relay stopped:

  ```
  docker run --rm -v infra_relay-data:/data debian:bookworm-slim chmod 600 /data/node.key
  ```

- Circuit limits come from `MAX_CIRCUITS`, `MAX_CIRCUITS_PER_PEER`,
  `MAX_CIRCUIT_SECS` and `MAX_CIRCUIT_BYTES`. The compose file sets them to
  **256 circuits, 8 per peer, 1 h and 32 MiB per circuit** (decision R11;
  libp2p's defaults are 16 / 4 / 2 min / 128 KiB). An empty value in `.env`
  means libp2p's default. The relay logs the values in effect at startup.
  32 MiB is about 8 times a 10,000-message catch-up (below) and stays this
  low on purpose: until sync pulls are gated on membership (GAP-4), the
  relay carries circuits for any peer. Review after the alpha using the
  `stats` line (`circuits_denied`, `circuits_open`).
- **Rate limits and the reverse proxy (`PROXY_IPS`).** libp2p limits
  reservations and new circuits per peer (30, then one every 2 minutes) and
  per source IP (60, then one a minute). Every wss client reaches the relay
  through the reverse proxy, so the relay sees them all as **one IP**. A
  shared per-IP bucket then starves everyone, and joins between testers fail
  once it's empty. Put the proxy hop's address, as the relay sees it (on this
  PC: the NPM host on the LAN), in `PROXY_IPS` in `.env`. Only those
  addresses skip the **per-IP** limits. The per-peer limits, the global
  circuit limits above, the connection limits and the memory cap all still
  apply, and so does the per-IP limit for everything on the direct 4001
  ports. Leave it empty if no proxy sits in front of the relay.

  The per-IP protection for wss then has to come from the edge. **Owner
  step:** in Cloudflare, add a rate-limiting rule on `<relay-domain>` for
  WebSocket upgrades per client IP (e.g. 20 per minute, block for 10
  minutes), then check that the app still connects (`wss`) afterwards.
  After a deploy, watch the `stats` line: `circuits_denied` climbing again
  means a flood or a too-tight limit.
- When the relay accepts a reservation, it puts the peer's circuit address
  (`<external>/p2p/<relay>/p2p-circuit`) into the DHT. Without this, a peer
  reachable only through the relay was usually not findable: the DHT only
  learned its direct addresses (identify runs before the reservation).
  Kademlia's routing table keeps addresses with no time limit (its
  `record_ttl` applies to stored records, not to peer addresses), so the
  relay removes the circuit addresses itself when the reservation times
  out or the peer's last connection closes, which ends the reservation.
- `EXTERNAL_ADDRS` must not be empty: reservations then carry no address and
  no client can listen on a circuit. The relay warns at startup.

### Relay-only catch-up (measured)

[`test/relay-catchup.sh`](test/relay-catchup.sh): two sync peers on separate
Docker networks that share only the relay, so a circuit is their only path.
p1 writes 10,000 messages while p2 is off, then p2 starts. Result on one PC
(2026-11-03, Docker Desktop, `sync-peer` from main):

| Circuit limits | Result |
| --- | --- |
| libp2p defaults (128 KiB, 2 min) | 0 / 10,000 after 60 s: every circuit is cut during the first page |
| `MAX_CIRCUIT_BYTES` 1 MiB | 8,000 / 10,000 after 45 s: each new circuit gets about one page (2,000 messages) before it is cut |
| 4 MiB | 10,000 in 1.0 s |
| 16 MiB | 10,000 in 1.2 s |
| 256 MiB, 1 h (first draft) | 10,000 in 0.9–1.2 s (3 runs) |
| compose values (R11: 32 MiB, 1 h, 8 per peer) | 10,000 in 1.1–1.2 s (3 runs) |

So a 10,000-message catch-up is about 4 MB through the relay. Before the
reservation fix above, only 1 run in 5 formed a circuit at all. There is no
WAN latency in this setup; over the internet, add a few round trips per
page (5 pages). Run it again with:

```
bash infra/test/relay-catchup.sh                       # the compose file's limits
MAX_CIRCUIT_BYTES= MAX_CIRCUIT_SECS= bash infra/test/relay-catchup.sh   # libp2p's defaults for those
```

It uses its own containers, networks and ports and leaves the live relay
alone.

### Reproducible builds

`relay/Cargo.lock` is committed and the image builds with `--locked`, so a
rebuild uses exactly the reviewed dependency versions. The image build also
runs the relay's tests. Base images (`rust`, `debian`, `caddy`, `coturn`) are
pinned by digest. To update one, take the new digest from
`docker buildx imagetools inspect <image:tag>`, then update the
`Dockerfile`/compose line in a PR. Update dependencies with
`cargo update` in `relay/` in a PR of their own; the CI licence job checks
the result.

## Making it public

A reverse proxy only carries HTTP and WebSocket traffic, so it covers part of it:

| Port | Protocol | How it reaches the internet |
| --- | --- | --- |
| 443 (+ 80 for certificates) → 4002 | WebSocket (`wss`, for browsers) | Caddy, below |
| 4001 | TCP | Port forwarding on the router |
| 4001 | UDP (QUIC) | Port forwarding on the router |
| 3478, 49160–49200 | UDP/TCP (TURN, phase 2) | Port forwarding on the router |

### wss with Caddy (automatic TLS)

The `caddy` service in the compose file proxies a whole subdomain to the
relay's WebSocket port and gets its certificate from Let's Encrypt by
itself. Config: [`caddy/Caddyfile`](caddy/Caddyfile).

1. Pick a subdomain, e.g. `relay.example.org` (placeholder), and point its
   DNS `A` record at your home IP.
2. Forward TCP **80** and **443** on the router to this PC. Port 80 is only
   used for the certificate challenge and redirects.
3. In `.env`: `RELAY_DOMAIN=relay.example.org`, and add
   `/dns4/relay.example.org/tcp/443/wss` to `EXTERNAL_ADDRS`.
4. `docker compose --profile proxy up -d`, then
   `docker compose logs caddy` should show "certificate obtained successfully".

Browsers then dial `/dns4/relay.example.org/tcp/443/wss/p2p/<peer_id>`. The
relay's port 4002 stays bound to `127.0.0.1` on the host (`WS_BIND`): Caddy
reaches it over the internal Docker network, nothing else can.

If you already run a reverse proxy on this PC instead, the equivalent is:

```
relay.example.org {
    reverse_proxy 127.0.0.1:4002
}
```

### wss with Nginx Proxy Manager (proxy on another machine)

When the proxy runs elsewhere on the LAN (for example NPM in a VM), leave
the `caddy` profile off and:

1. In `.env`, set `WS_BIND` to this PC's LAN address so the proxy can reach
   port 4002. Then allow 4002/tcp in Windows Firewall **only from the
   proxy's address**; no other machine needs it.
2. In NPM, **Hosts → Proxy Hosts → Add Proxy Host**:
   - Domain Names: `<relay-domain>`
   - Scheme `http`, Forward Hostname/IP: this PC's LAN address, Forward
     Port `4002`
   - **Websockets Support: on** (without it the upgrade fails and browsers
     get no connection). Block Common Exploits: on. Cache Assets: off.
   - SSL tab: request a Let's Encrypt certificate, Force SSL on, HTTP/2 on.
   - Advanced tab, custom Nginx configuration:

     ```
     access_log off;
     proxy_read_timeout 1h;
     proxy_send_timeout 1h;
     ```

     `access_log off` keeps visitors' IPs off NPM's disk (it logs every
     request by default). The timeouts stop nginx's 60 s default from
     cutting quiet WebSocket connections; libp2p pings every 15 s, so this is
     a safety margin.
3. Router: forward 443 (and 80, for the certificate) to the NPM machine, and
   4001 TCP + UDP to this PC.
4. `EXTERNAL_ADDRS` in `.env` lists `/dns4/<relay-domain>/tcp/443/wss` and
   the 4001 addresses.

Check from outside your network: `https://<relay-domain>/` should answer
(an error page from the relay's WebSocket port is fine: TLS and the proxy
work), and a browser client dialing
`/dns4/<relay-domain>/tcp/443/wss/p2p/<peer_id>` connects.

Things to know about hosting at home:

- Peers can only bootstrap while this PC is on.
- The domain points at your home IP, so that IP is public. A proxy service that
  hides it (like Cloudflare's) does not carry the TCP/UDP ports above.

## Running on a Linux host (real source addresses)

Hole punching (DCUtR, #219) needs the relay to see each client's real public
address. Docker Desktop on Windows can't do that: every inbound connection to
a published port, the forwarded 4001 included, arrives from the Docker
gateway (`172.18.x.1`). Run the relay on a Linux host with a bridged network
interface instead, such as a small Proxmox VM or LXC (1 vCPU, 512 MB), and
use host networking. It's the same image, compose file and hardening.

1. **Host**: Debian with Docker Engine (not Docker Desktop), on a bridged NIC
   with a fixed LAN address. For an LXC, enable `nesting` so Docker runs.
2. **Move the identity**: back up `node.key` on the old PC and restore it on
   the new host ([Backing up the relay key](#backing-up-the-relay-key)), so the
   peer ID, and every built-in relay address, stays the same. Stop the old
   relay first, so two relays never run with one key.
3. **`.env`** on the new host: copy the old one, then:
   - `WS_BIND` doesn't apply (host networking binds 4002 on the host);
   - `PROXY_IPS` = the reverse proxy's LAN address. The relay now sees NPM's
     real address instead of the Docker gateway, so this changes;
   - `EXTERNAL_ADDRS` keeps the wss address. When the owner's DNS-only
     hostname for TCP exists, add `/dns4/<relay-tcp-host>/tcp/4001` and
     `/dns4/<relay-tcp-host>/udp/4001/quic-v1` (the same name goes into the
     repository variable `MVS_RELAY_TCP_ADDR` for clients; never commit it).
4. **Start** with the host-network override:

   ```
   docker compose -f docker-compose.yml -f docker-compose.host.yml up -d --build relay
   ```

5. **Firewall on the host**: 4001 TCP+UDP open; **4002 only from the reverse
   proxy** (`nft` or `ufw allow from <npm-lan-address> to any port 4002 proto tcp`).
6. **Router**: move the 4001 TCP+UDP forward to the new host's LAN address.
7. **NPM**: change the relay proxy host's Forward Hostname/IP to the new
   host's LAN address (port 4002, WebSockets on, as above).
8. **Check**: from a phone hotspot (outside your network), start a client and
   run `docker compose logs relay` with `RUST_LOG=info,relay_node=debug` for a
   minute, then set it back. The connection's remote address must be the
   hotspot's public IP, not a private one. Also check that `circuits_denied` in
   the `stats` line stays flat.

The Caddy profile isn't supported with this override: Caddy reaches the
relay by its compose service name, which host networking doesn't provide. Use
NPM, or point Caddy at `127.0.0.1:4002`.

## Uptime and metrics

Kept simple and private: nothing here publishes who connects.

- **Health**: `docker compose ps` (healthcheck above).
- **Stats line**: every `STATS_INTERVAL_SECS` (default 300) the relay logs
  counts only, no addresses or peer IDs:

  ```
  stats uptime_secs=3600 peers=12 connections=14 circuits_open=2 circuits_accepted=40 circuits_denied=0 reservations_accepted=9 reservations_denied=0
  ```

  `docker compose logs relay | findstr stats` (PowerShell: `Select-String stats`)
  shows the history, within the log-rotation window. Per-connection lines
  (with peer IDs) are only logged at `RUST_LOG=debug`. Keep `info` in normal
  operation, and don't post debug logs publicly: libp2p's debug output can
  contain peers' IP addresses.
- **From outside**: to see whether testers can reach the relay, run a TCP
  check against `<domain>:4001` from a machine outside your network (for
  example Uptime Kuma on another PC, or a hosted TCP monitor). It only learns
  that the port is open. Keep any status page private: it would show when
  your PC is off.
- **Caddy** keeps no access log (there is no `log` directive), so visitor IPs
  are not written to disk. With NPM, the `access_log off;` line above does
  the same.

### What reaches the disk

No connection logs. At `RUST_LOG=info` the relay writes its startup lines
(own peer ID, listen addresses, circuit limits), warnings and the counts-only
`stats` line, and nothing per connection. Who is connected to whom lives
only in the process's memory and is gone on restart. Docker's `json-file`
log keeps those lines (rotated, at most 50 MB). `RUST_LOG=debug` is for
short troubleshooting only, because it writes peer IDs and possibly IP
addresses into that file; set it back to `info` and recreate the container
(`docker compose up -d relay`), which also drops the old log.

## Backing up the relay key

`relay-data` holds `node.key`, the relay's identity. If it is lost, the
relay gets a new peer ID on the next start and every client's built-in
bootstrap address stops working until a new release ships. Anyone who has
the key can impersonate the relay, so treat the backup as a secret: keep it
offline (the same drive as the update keys is fine), never in git or a
synced folder.

Back up (PowerShell, from this folder; the volume is `infra_relay-data`
when started from `infra/`):

```
docker run --rm -v infra_relay-data:/data:ro -v "${PWD}:/backup" debian:bookworm-slim tar -czf /backup/relay-key.tgz -C /data node.key
```

Then move `relay-key.tgz` to the offline drive and delete it here (git
ignores it, but it should not stay on this PC).

Restore (for example on a new PC, before the first `docker compose up`):

```
docker volume create infra_relay-data
docker run --rm -v infra_relay-data:/data -v "E:\backup:/backup:ro" debian:bookworm-slim sh -c "tar -xzf /backup/relay-key.tgz -C /data && chown 10001 /data/node.key && chmod 600 /data/node.key"
```

Check: `docker compose logs relay` must show the same `peer_id` as before.

Caddy's volumes (`caddy-data`) only hold certificates it can get again;
they don't need a backup.

## Enabling Docker on this PC

Docker Desktop needs the Windows "Virtual Machine Platform" feature. In an
administrator terminal:

```
wsl --install --no-distribution
```

Then restart Windows and start Docker Desktop.
