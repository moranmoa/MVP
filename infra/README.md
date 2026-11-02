# Helper servers

These containers never store or read messages. They only help computers find
each other and pass along encrypted traffic when a direct connection fails.

| Service | What it does | Needed from |
| --- | --- | --- |
| `relay` | Bootstrap (DHT entry point) and circuit relay | Closed alpha |
| `coturn` | TURN relay for voice and the Web client | Phase 2 (`--profile voice`) |

The update seeder joins here together with the updater in the MVP.

## Run on this PC

1. Docker Desktop must be running (it needs WSL 2, see below).
2. `copy .env.example .env` and fill in your domain. `.env` is git-ignored.
3. From this folder:

   ```
   docker compose up -d --build
   docker compose logs relay
   ```

   The log line `relay node started peer_id=...` shows the node's ID. Clients
   use `/dns4/<domain>/tcp/4001/p2p/<peer_id>` as the bootstrap address. The
   ID is stable: the key lives in the `relay-data` volume, not in git.

## Making it public

A reverse proxy only carries HTTP and WebSocket traffic, so it covers part of it:

| Port | Protocol | How it reaches the internet |
| --- | --- | --- |
| 443 → 4002 | WebSocket (`wss`, for browsers) | Reverse proxy with your domain and TLS |
| 4001 | TCP | Port forwarding on the router |
| 4001 | UDP (QUIC) | Port forwarding on the router |
| 3478, 49160–49200 | UDP/TCP (TURN, phase 2) | Port forwarding on the router |

Reverse proxy example (Caddy), WebSocket on a path of your domain:

```
chat.example.com {
    reverse_proxy /p2p* 127.0.0.1:4002
}
```

If you use a path like `/p2p`, add it to the `wss` address in `EXTERNAL_ADDRS`.

Things to know about hosting at home:

- Peers can only bootstrap while this PC is on.
- The domain points at your home IP, so that IP is public. A proxy service that
  hides it (like Cloudflare's) does not carry the TCP/UDP ports above.

## Enabling Docker on this PC

Docker Desktop needs the Windows "Virtual Machine Platform" feature. In an
administrator terminal:

```
wsl --install --no-distribution
```

Then restart Windows and start Docker Desktop.
