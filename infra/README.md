# Helper servers

These containers never store or read messages. They only help computers find
each other and pass along encrypted traffic when a direct connection fails.

| Service | What it does | Needed from |
| --- | --- | --- |
| `relay` | Bootstrap (DHT entry point) and circuit relay | Closed alpha |
| `caddy` | `wss://` with automatic TLS in front of the relay | When browsers join (`--profile proxy`) |
| `coturn` | TURN relay for voice and the Web client | Phase 2 (`--profile voice`) |

The update seeder joins here together with the updater in the MVP.

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

Things to know about hosting at home:

- Peers can only bootstrap while this PC is on.
- The domain points at your home IP, so that IP is public. A proxy service that
  hides it (like Cloudflare's) does not carry the TCP/UDP ports above.

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
  are not written to disk.

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
