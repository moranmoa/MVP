# Voice rooms – "Join to create"

- **Source:** owner request (2026-11-11), modelled on the owner's Discord bot `moranmoa/TGC-bot` (`src/events/voiceStateUpdateEventHandler.js`). The bot's room buttons (`voiceChannelButtons.js`) are commented out; the only planned button was "Edit name", for the room's master only.
- **Replaces:** the single voice room per server built in #239 (V7). Everything else from #224, #239, #300 and `voice-ip-notice.md` stays: the call bar, mute, the 5-person mesh limit, relay-only blocking, the in-call-elsewhere reason, and the one-time IP notice.
- **Screenshots:** 380–389.

## 1. Model

| Bot behaviour | MVS |
| --- | --- |
| Joining the root "Join chat" channel creates a new voice channel and moves you into it | Clicking **"הצטרפות לשיחה"** creates a new room and joins you to it. The row is always there, whatever rooms exist |
| Room named after the creator's game or activity | No activity data in MVS. Default name **"החדר של ⁨{name}⁩"** / **"⁨{name}⁩'s room"**. The master can rename it |
| The creator is the room's master | Same. A crown icon next to the master in the room's member list |
| Master leaves → the next member (earliest joined) becomes master, and the room is renamed after them | Same hand-off rule. The room is renamed **only if it still has the default name**; a custom name stays |
| Empty room is deleted | Same, at once. Nothing is stored |
| Others join from the channel list | Same: click a room row to join it |

## 2. Sidebar

```
 ערוצים                        +
 # כללי
 # clips
 ─────────────────────────────
 🔊 הצטרפות לשיחה               ← pinned, always shown; creates a new room
    פותח חדר חדש                ← secondary line, only while ≥1 room exists
 🔊 החדר של נועה         3/5    ← a room: click to join
    👑 נועה
       אביב  🔇
       דנה
 🔊 גיימינג                5/5  ← full: muted text, not clickable
    👑 עומרי  …
```

- **Root row:** label `call.room` (existing, unchanged per the owner). While at least one room exists, a muted second line `voiceRooms.createHint` says that clicking it opens a new room, so nobody creates a room by mistake when they meant to join one.
- **Room rows:** speaker icon, room name (`<bdi>`, one line, ellipsis), and the count `n/max` (`<bdi dir="ltr">`). Under each room, its members as on #239 (avatar, name, mute icon, `peerFailed`/`peerRefused` notes), with a crown on the master.
- **Order:** by creation time, oldest first. Rooms aren't reorderable (they're temporary).
- **The room I'm in** is highlighted (`--bg-active`) and labelled `call.roomIn` in its accessible name.
- **More than 8 rooms:** rooms beyond 8 collapse under "עוד ⁨{count}⁩ חדרים" (`voiceRooms.more`).

## 3. Actions

| Action | Who | Behaviour |
| --- | --- | --- |
| **Create** | Members (not in relay-only) | Click the root row → IP notice the first time (`voice-ip-notice.md`) → a new room named `voiceRooms.defaultName` with me as master → I'm in it. If I'm already in another room, I leave it first, without a confirmation (like Discord). |
| **Join** | Members | Click a room row → join. Full room: the row is `aria-disabled` with `call.roomFull`. Same IP-notice and in-call-elsewhere rules as today |
| **Leave** | Everyone in the room | The existing hang-up in the call bar. If I was master, hand-off (§4) |
| **Rename** | Master, and the server owner | Room context menu (B25) → "שינוי שם" → inline field, max 32, Enter saves, Esc cancels. An empty name restores the default |
| **Set limit** | Master | Room context menu → "מגבלת משתתפים" → 2, 3, 4 or 5 (the mesh maximum). Can't go below the current count |
| **Room menu, everyone** | – | "הצטרפות" (or "יציאה" if I'm in it), plus "העתקת שם החדר" |

- **No lock, no kick-from-room, no transfer of master** in v1. The bot had none. A member can be removed from the server as today (B11 / B25).
- **Phone:** the rooms sit in the channel list. Tap to join; long-press for the room menu (B25 bottom sheet).
- **Keyboard:** the root row and room rows are in the channel list's roving tabindex. Enter joins or creates. Shift+F10 opens the room menu.

## 4. Master hand-off and disappearance

- **Hand-off:** when the master leaves (or disconnects for longer than the presence timeout, about 15–30 s, per B18 §2א), the member who joined the room earliest becomes master. Ties go to the smaller account id. Every client computes the same answer from the same join order.
- **Renaming on hand-off:** only if the name is still the default (`החדר של ⁨{old}⁩` → `החדר של ⁨{new}⁩`). A custom name stays.
- **Announcement:** the screen reader announces `voiceRooms.newMaster` (polite) to people in the room. No toast.
- **Empty room:** it disappears as soon as the last member leaves. Nothing is kept: no name history, no log entry.

## 5. Limits and states

| State | What you see |
| --- | --- |
| Room full (n = limit) | Count in `--danger`, the row is `aria-disabled`, and its tooltip and accessible name include `call.roomFull` |
| Relay-only on | Root row and room rows disabled, with the existing reason line and "שינוי בהגדרות" (#273) |
| In a call on another computer | The existing `call.error.inCallElsewhere` line (#300) |
| No rooms | Only the root row, without the second line |
| Server closed | No voice section at all (#264) |
| A room's master is offline (presence) | Hand-off as in §4 after the timeout. Until then, the crown stays |
| Too many rooms (server cap, see the tech note) | Root row disabled with `voiceRooms.capReached` |

## 6. Tech-lead note (not UX decisions; for #239's owner)

- Rooms are **ephemeral and member-gated**. They should ride on the existing voice signalling and presence, not on signed channel records. Nothing goes in the server log, and nothing syncs to people who join later.
- Room state that clients need: `{ roomId, name, limit, createdAt, members: [{ userId, joinedAt, muted }] }`, with the master derived from `joinedAt` (§4), not stored. A rename or limit change is a signalling message from the master or the server owner; other clients reject it from anyone else.
- The room name is member data: encrypt it like other server content, and keep it off the relay.
- **Suggested server cap:** 10 rooms at once, so a single member can't flood the sidebar. The exact number is the tech lead's call.

## 7. Strings

| Key | Hebrew | English |
| --- | --- | --- |
| `call.room` | `הצטרפות לשיחה` (unchanged) | `Join chat` (unchanged) |
| `voiceRooms.createHint` | `פותח חדר חדש` | `Opens a new room` |
| `voiceRooms.defaultName` | `החדר של ⁨{name}⁩` | `⁨{name}⁩'s room` |
| `voiceRooms.rename` | `שינוי שם` | `Rename` |
| `voiceRooms.limit` | `מגבלת משתתפים` | `Member limit` |
| `voiceRooms.limitValue` | `עד ⁨{n}⁩` | `Up to ⁨{n}⁩` |
| `voiceRooms.join` | `הצטרפות` | `Join` |
| `voiceRooms.leave` | `יציאה` | `Leave` |
| `voiceRooms.copyName` | `העתקת שם החדר` | `Copy room name` |
| `voiceRooms.master` | `ניהול החדר` | `Room host` |
| `voiceRooms.newMaster` | `ניהול החדר עבר ל-⁨{name}⁩` | `⁨{name}⁩ is now the room host` |
| `voiceRooms.more` | `עוד ⁨{count}⁩ חדרים` | `⁨{count}⁩ more rooms` |
| `voiceRooms.capReached` | `יש כבר הרבה חדרים בשרת. אפשר להצטרף לאחד מהם.` | `This server already has many rooms. Join one of them.` |
| `voiceRooms.roomLabel` | `⁨{name}⁩, ⁨{count}⁩ מתוך ⁨{max}⁩` | `⁨{name}⁩, ⁨{count}⁩ of ⁨{max}⁩` |

Note: the crown's accessible name is `ניהול החדר`, and `newMaster` is phrased as "ניהול החדר עבר ל-…". Both are noun phrases, so neither is gendered.

## 8. Acceptance criteria

1. With no rooms, clicking "הצטרפות לשיחה" creates "החדר של {me}" with me in it and the crown on me.
2. A second member clicks the room row and joins it; the count shows 2/5. A third member clicks the root row and gets a **new** room, so two rooms are listed.
3. The root row shows "פותח חדר חדש" only while at least one room exists.
4. The master leaves a 3-person room: the earliest-joined remaining member gets the crown, the default name changes to their name, and `newMaster` is announced to the room.
5. If the master had renamed the room to "גיימינג", the hand-off keeps the name "גיימינג".
6. The last member leaves and the room disappears for everyone.
7. A room at its limit is `aria-disabled`, with "The chat is full" in its accessible name; clicking does nothing.
8. Only the master and the server owner see "שינוי שם" and "מגבלת משתתפים"; other members don't (not in the DOM).
9. The limit can't be set below the current member count (those options are disabled).
10. While I'm in room A, clicking room B leaves A and joins B. I'm in only one room at a time.
11. In relay-only mode the root row and all rooms are disabled, with the reason and "שינוי בהגדרות".
12. The one-time IP notice appears before the first create or join, not on later ones.
13. On a phone (375px), long-press on a room opens the bottom-sheet menu, and tapping joins.
14. With 11 rooms, 8 are listed and "עוד 3 חדרים" expands the rest.
15. Two clients compute the same master after a hand-off (same `joinedAt` order, ties by id).

**Screenshots:**

| # | Shot | What it shows |
| --- | --- | --- |
| 380 | `380-voice-rooms-two-he` | Root row with the hint, and two rooms with members and the crown |
| 381 | `381-voice-room-full-he` | A full room (5/5) disabled |
| 382 | `382-voice-room-menu-master-he` | The master's room menu: rename and limit |
| 383 | `383-voice-room-rename-he` | Inline rename |
| 384 | `384-voice-rooms-phone-he` | 375px: rooms in the channel list |
| 385 | `385-voice-rooms-en-light` | 380 in English, LTR |
