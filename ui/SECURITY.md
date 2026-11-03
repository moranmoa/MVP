# UI security notes

Threat model: everything the UI shows from the core may have been written by
another person. That covers display names, server and channel names, message
text, bios, avatar images, avatar and server colours, invite links and the
update version. The UI must show all of it as plain, honest text: it must not
run it, load anything because of it, or let it pretend to be someone else.

## Where the defences live

| Defence | Where |
| --- | --- |
| Every core result and event is cleaned once, before any screen sees it | `src/core/sanitize.ts` (`guardCore`), applied in `src/app/App.tsx` |
| No HTML injection, no `eval`, no logging, no links built from data | `src/test/security.test.tsx` (source rules) |
| Content-Security-Policy | `csp.ts`, added to the built `index.html` by `vite.config.ts` |
| Recovery words never stored, logged or copied | `src/core/mock.ts`, `src/screens/onboarding/*` |
| Look-alike names | `nameSkeleton` in `sanitize.ts`, `lookalikeIds` in `src/screens/shell/MemberList.tsx` |
| Licences | `npm run licenses` (`scripts/check-licenses.mjs`) |
| Browser checks | `docker compose run --rm shots` fails on any CSP violation, page error, hostile input that becomes markup, recovery word in storage, or sideways overflow at 375 px |

`?demo=1&hostile=1` loads the mock with attacker-style names and messages, so
you can check the defences by hand.

## What the cleaners do

- **Names** (people, servers, channels): NFKC (so fullwidth and math letters
  become plain letters). Direction overrides, isolates and marks are removed,
  as are invisible and "blank" characters (zero-width, fillers, tags, most
  variation selectors), stray zero-width joiners (emoji sequences are kept),
  and identity-code look-alikes such as `Moran K7QM-2XPA`. A letter keeps at
  most 4 combining marks (enough for Hebrew vowels, not for "zalgo"). The
  result is one line of at most 32 characters (servers 48). If nothing visible
  is left, the identity code is shown instead.
- **Message text and bios**: the same, but newlines and LRM/RLM are kept.
  Messages are capped at 4,000 characters and 100 lines and get a "too long"
  note. Bios are capped at 300 characters and 8 lines. Reply previews are one
  line of 140 characters.
- **Colours**: only `#rrggbb`. They end up in inline CSS, where `url(...)`
  would load a remote image and reveal the reader's IP address.
- **Avatars**: only `data:image/(png|jpeg|webp|gif);base64,…` up to about
  512 KB. No remote URLs (IP leak) and no SVG.
- **Invite links**: one line, no invisible or direction characters, at most
  512 characters.
- **Enums and numbers**: unknown age limits become `18+`, unknown presence
  becomes offline, counters are clamped.

React escapes all text, and nothing in `src/` uses `dangerouslySetInnerHTML`
or builds a link from data, so markup, `javascript:` URLs and markdown stay
inert text. A test enforces this.

## Content-Security-Policy

The production build ships this policy as a `<meta>` tag (from `csp.ts`):

```
default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:;
font-src 'self'; connect-src 'self'; media-src 'self' blob:; manifest-src 'self';
worker-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'
```

The dev server runs without it, because hot reload needs inline scripts.
`frame-ancestors` cannot be set in a meta tag; the shell has to send it.

**Tauri** (`tauri.conf.json`): set `app.security.csp` to the same directives,
with `connect-src 'self' ipc: http://ipc.localhost`, and leave
`dangerousDisableAssetCspModification` off so Tauri can add its nonces. Keep
the capability list small. The UI needs only the core commands, not
`shell:open`, `fs` or `http`.

**Electron**: set these on the window: `contextIsolation: true`,
`nodeIntegration: false`, `sandbox: true` and `webSecurity: true`. Serve
`dist/` from a custom `app://` protocol (`protocol.handle`) and send the
policy plus `frame-ancestors 'none'` as a response header. Deny
`setWindowOpenHandler` and `will-navigate`, and expose the core through a
`contextBridge` that has one function per command.

## Recovery words

- They are shown once, as a numbered grid that cannot be selected or copied.
  The words are meant for paper, and copying them would leave them in the
  clipboard and its history.
- After the 3-word check (or "later"), the words are dropped from the UI's
  memory.
- Restore accepts a paste of all 12 words, then makes a best-effort attempt
  to clear the clipboard.
- The mock no longer writes them to `localStorage`; it rebuilds them from the
  account id. Old saved data that still has them is cleaned when it loads. The
  real core keeps them in the OS keychain (see below).
- Nothing in `src/` logs. The diagnostics export never contains message text,
  words or the birth date.

## Dependencies

`npm audit` finds nothing (vitest was moved to 4.1.12 because 3.x pulled in a critical `tinypool` advisory), and the shipped bundle is MIT,
BSD-3-Clause and 0BSD only. To check dev and shipped licences against the
allow-list, run:

```bash
docker compose run --rm ui npm run licenses
```

The CI ui job can run `npm audit --audit-level=high` and `npm run licenses`.

## Needs the real core

These cannot be settled in the UI. The core (or the shell) must do them:

1. **Recovery words** live in the OS keychain (Windows Credential Manager,
   macOS Keychain, Secret Service), never in a file or the webview's storage.
   `getRecoveryWords` should require a fresh OS authentication.
2. **Identity codes** come from public keys that the core has verified. The UI
   only displays the code it is given.
3. **Size limits are enforced when data arrives**, before storage: messages
   (4,000 characters), names (32), avatars (512 KB, decoded and re-encoded
   so that image-parser bugs never reach the webview). The UI limits are
   only a second line of defence.
4. **Roles and the owner badge** come from the signed control log and never
   from a display name. The look-alike warning helps people notice an
   impersonator, but it is not a check.
5. **Invite links** are parsed and validated by the core. The UI treats them
   as opaque text.
6. **Birth date** is encrypted at rest by the core. The mock keeps it in
   `localStorage` only for the demo.
7. **The diagnostics export** is assembled by the core, which must never add
   message text, words, keys or the birth date to it.
8. **The update version string** comes from the signed update manifest.
9. **The shell's CSP and window hardening** are listed above.
10. **Rate limits** on messages, joins and member-list churn, so that one peer
    cannot flood the UI with events.
