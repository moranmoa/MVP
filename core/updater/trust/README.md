# Embedded update trust root

`root.json` is the **public** update root that every release embeds
(`updater::embedded_root()`), so installed apps can verify signed manifests.
It holds public keys only. The secret key stays offline with the owner and
is never committed.

- v1: one owner key, threshold 1, for the closed alpha (created with
  `updater keygen` and `updater make-root --threshold 1` from main `ef15844`).
- The move to 2-of-3 on hardware keys (spec, before the open alpha) is a
  signed rotation (`updater sign-root --rotation`), published as
  `root-2.json` with the manifests. Clients apply it with `update_chain`. This
  file changes only so that new installers start from a newer root; old
  clients follow the signed chain.

Changing this file changes which keys can ship code to every user. Any PR
that touches it needs the tech lead **and** the security lead, and the test
`embedded_root_is_the_owners_v1_key` must be updated in the same PR.
