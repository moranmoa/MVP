# Profile picture and colour for other members

- **Status:** proposal, dev-identity (2026-11-11), for review by the Tech Lead and Security Lead.
- **Owner request:** my profile picture and my colour show everywhere and for everyone, not just on my own card. Today messages and the member list show a letter on a colour derived from my ID.

## The constraint
- **Where it goes:** the profile goes to my servers as a signed control-log `Post` (`mvs/profile/v1`, #296).
- **The limit:** a `Post` body is capped at **4 KiB** (`records::Kind::max_body`). Raising it, or adding a new record kind, is a records change that every 0.3.x/0.4.x peer refuses, and the later control records that depend on it stall there (#234 §8, as for G-11).
- **So:** a 64 KiB picture can't travel in the profile post. It has to fit in it.

## Design: a small picture inside the profile post
- **Profile body** (additive, older clients ignore the new fields):
  ```json
  {"mvs":"mvs/profile/v1","displayName":"…","caps":[…],
   "avatar":"data:image/webp;base64,…","color":"#3b82f6"}
  ```
- **`avatar`:** the UI makes it from the picture already on my card. It's square, **48×48**, WebP (PNG as a fallback), shown at 32–40 px.
  - **Hard cap: 2,560 bytes of image**, about 3.4 KB of base64. That leaves room in the 4 KiB post for the name (128 characters at most), the caps and the colour.
  - A 48×48 WebP at quality 70 is usually 1–2 KB.
- **`color`:** `#rrggbb`. It replaces the derived colour behind the letter when there's no picture, and rings the picture. The UI offers the palette plus a custom value; the core accepts any `#rrggbb`.
- **Size of the encoding step:** the core never decodes images. The UI encodes (canvas → WebP) before calling `updateProfile`.

## The core (api)
- **`updateProfile` input:** gains `avatarSmall?: string` (the 48×48 data URL) and `color?: string`. Both are stored with the profile and published in the next profile post, as the name is (#296).
  - The post goes out only when the name, `avatarSmall` or `color` changed.
  - The large `avatarDataUrl` stays on this device, as today.
- **Validation, on send and on every received post (untrusted):**
  - `avatar`: `data:image/(webp|png);base64,`, valid base64, decoded size ≤ 2,560 bytes.
  - It must start with the format's magic bytes: `RIFF….WEBP`, or the PNG signature. No SVG, no other types.
  - Anything else: the avatar is ignored (the name still counts). On send, it's `bad-request`.
  - `color`: exactly `#` plus 6 hex digits; otherwise ignored.
- **Decoding safety:** the core never decodes pixels; it only checks the size, the type and the magic bytes. The UI shows the image with `<img src="data:…">`, decoded by the browser's sandboxed image decoders. Nothing is fetched from anywhere.
- **Exposed** (additive):
  - `Member.avatarDataUrl?`, `Member.color?`;
  - `Message.authorAvatar?`, `Message.authorColor?`, or the UI looks the author up in `listMembers`. The Tech Lead picks; the leaner option is `Member` only, plus a `members.changed` event when a profile post arrives.
  - The latest valid profile post per member wins, as for names today.
- **Rate of updates:** each change is one ~4 KiB control record in every server I'm in, kept for good. Limits:
  - **Core:** no post when nothing changed; at most **one profile post per server per 10 minutes**, with the latest change published when the 10 minutes end (coalesced).
  - **Receivers:** they take the latest anyway. A flood from a modified client costs storage, as any control post spam does today (GAP-8, quotas), and gains nothing on display.
- **Privacy:** the picture and colour go only to my servers' members, like the name. In sync's store the control log is plaintext today (GAP-11).

## Tests (in the code PR)
- **Round trip:** set a picture and colour; another member sees them on `Member`.
- **Limits:** a picture over 2,560 bytes, an SVG, a bad base64, or a PNG with WebP magic gives `bad-request` on send, and is ignored when received in a crafted post.
- **Colour:** a bad value is ignored.
- **Posting rules:** no post when nothing changed; a second change within 10 minutes is coalesced into one post.
- **Old clients:** a profile post without `avatar` or `color` still gives the name.

## UI (dev-ui, after this, sanitizer first)
- Keep `Member.avatarDataUrl` only if it's a `data:image/(webp|png);base64,` URL of at most about 3.5 KB, and `Member.color` only as `#rrggbb`.
- Show the picture everywhere, with the colour ring or the letter on that colour.
- Settings → Account gets a colour picker. Making `avatarSmall` (canvas → 48×48 WebP, quality lowered until it's 2,560 bytes or less) on save is the UI's job.

## Decisions (Tech Lead, on #354 at 1215e55)
- **Exposed on `Member` only:** `avatarDataUrl?` and `color?`, plus `members.changed` when a profile post arrives. No author fields on `Message`.
- **The whole post is checked:** the serialized profile post is checked against `max_body` (4 KiB) before it's published. A maximum-length Hebrew name plus a maximum picture can go over; that's `bad-request` on send.
- **Absent versus empty:** a post without `avatar` or `color` leaves the earlier value. `""` clears it. That way an older device, or the restore-and-rejoin path, never wipes the picture.
- **The pending publish survives a restart:** the coalesced 10-minute post is stored, so a restart doesn't lose it.
- **Size before decoding:** the base64 length is checked before anything is decoded.
- **Restore keeps both:** a restore keeps both fields. They go into the backup blob later, with the profile (#291).
