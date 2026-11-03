# B6: "sent" and "delivered" from the core (design note)

**Status: approved by both leads (#124) and built.** Stacked on
#122 (same node code). Sources: the B6 UX spec (#102), ש-11, the
security lead on #105 ("the ack goes only to the author, once GAP-4
lands"), and the M2 criteria (row 3).

**Sent.** A record we wrote is *sent* once its bytes left on at least one
connection. That happens in one of two ways:
- gossipsub `publish` succeeded, so it was queued to at least one peer;
- if nobody was connected, a later pull answer to some peer included it.

The node keeps the ids of its own records that haven't gone out yet
(bounded, 10,000). It reports each one once with a new
`NodeEvent::Sent { id }`. `Node::append` keeps its signature, so api
doesn't break. api sets `pending` when it appends, and `sent` on the event.

**Delivered (receipt ack, ש-11).** The ack is temporary and lives at the
protocol level: it is never logged and never synced. It travels inside
the pulls the author already makes every 5 s, so it needs no new protocol
and no extra connection. The author's node also pulls the log about 1 s
after publishing, so "delivered" usually shows within a couple of seconds.
- **The storing side.** When a node stores a record from a peer, it
  remembers the record id under (server, author). The memory is bounded
  (256 ids per author, 4,096 authors) with a 10-minute TTL. When a peer
  pulls one of that server's logs with a valid GAP-4 binding, the answer
  carries `acks`: the remembered ids whose author is *the bound identity
  of the requester*. They are drained when sent. So an ack goes only to
  its author, as the security lead asked; anyone else gets none.
- **The author's side.** To count an ack as "a member has it", the answer
  also carries the answerer's binding, signed over the same control
  handle. The author checks that binding against the Noise remote and
  passes the identity through its own `ReadGate` (a member, not kicked),
  on the decoding thread. Then it emits `NodeEvent::Delivered { id }`
  once per id, and only for records it wrote.

This works only for servers opened with their secret (GAP-4). Logs opened
by plain name get "sent" but never "delivered". Like other node events,
these can be dropped if api lags (`Lagged`): a missed "delivered" leaves
the message at "sent", which is acceptable for a best-effort tick.

**Not in this PR:** api wiring (`SendState::Received` on the event), which
belongs to dev-identity together with the GAP-4 switch; and "failed +
retry", which stays in api (a failed `append`).

## Additions (tech lead, #124)

1. **Bounded acks.** At most 256 ack ids per answer. Any left wait for the
   next pull, still under the 10-minute TTL. A test pins the cap.
2. **Only stored records are acked.** An id is remembered only when the
   store says `Added`, or `Known` (we already hold it). A record that is
   waiting for deps, refused or invalid is never acked.
3. **Backoff jitter.** The relay redial backoff is jittered by ±20 %
   (commit `83dd33c`, unit-tested bounds).
4. **A fake ack is harmless.** An ack only moves a message from "sent" to
   "delivered" in its author's own UI. It changes nothing that is stored,
   synced or shown to anyone else. A peer that lies can make an author
   believe a message was delivered when it wasn't. It can already do the
   same by taking the record and then dropping it, so "delivered" means
   "a member's node said it stored it", not a guarantee. The binding and
   `ReadGate` check still keep non-members from producing it.

## Security lead's notes (#124)

- **"Sent" is not "a member has it".** It means the record left this
  device; it may have reached only a non-member relaying ciphertext. Only
  "delivered" means a member's node has it. api and the UI copy should
  say so.
- **An ack only drives the tick.** It is never used for authority,
  ordering or deletion. This is stated in the node's module docs and on
  `NodeEvent::Delivered`.

## As built

`NodeEvent::Sent { id }` and `NodeEvent::Delivered { id }` are each sent
once per record.
- **Acks in pull answers:** `SyncResponse` has `acks` (at most 256) and the
  answerer's `binding`. The memory is `Acks` in `limits.rs`: 256 per author,
  4,096 authors, 10 minutes. Only `Added` or `Known` records are remembered.
- **Pulling soon:** a log written to is pulled again within 1 s.
- **Tests:**
  - `acks_are_capped_per_answer_per_author_and_in_time`.
  - `a_message_is_sent_then_delivered_once_a_member_stored_it`, an
    end-to-end run. The message isn't sent while nobody is connected, nor
    to a bound non-member who may not read the channel. A member's pull
    makes it sent, then delivered. A message written with peers connected
    is sent at once, then delivered.
