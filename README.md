# Decentralized Discord replacement

A peer-to-peer chat and voice app: servers, channels, roles and voice rooms,
with no central server storing messages.

- `docs/spec.md` – product spec (Hebrew), exported from the living doc.
- `infra/` – helper servers (bootstrap/relay, TURN) that run in Docker.

Secrets (`.env`, keys) stay out of git; see `.gitignore`.
