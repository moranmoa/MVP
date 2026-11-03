# ui – MVP interface shell

React + TypeScript + Vite. A plain web app with no desktop-shell APIs, so it
can sit unchanged inside Tauri or Electron. Today it talks to a **mock core**
(`src/core/mock.ts`); the real Rust core will replace it behind the same
typed API (`src/core/api.ts`).

Node is not needed on the host. Everything runs in Docker (`node:22.23.4-bookworm`);
`node_modules` lives in a named volume.

## Commands

Run these from `ui/`:

```bash
docker compose run --rm ui npm install
```

```bash
docker compose up ui
```

The dev server is then at <http://localhost:5173/>.

```bash
docker compose run --rm ui npm run build
```

```bash
docker compose run --rm ui npm test
```

```bash
docker compose run --rm shots
```

`shots` captures the main screens into `screenshots/` with Playwright
(`mcr.microsoft.com/playwright:v1.56.1-noble`) and prints a rough
performance check (scrolling the 100k-message channel, server switch).

## Demo URL flags (mock core only)

| Flag | Effect |
| --- | --- |
| `?demo=1` | Signed in with three sample servers; one channel has 100,000 messages |
| `?demo=unchecked` | Same, but the recovery words are not checked (reminder banner) |
| `?fresh=1` | Forget the stored mock account and start onboarding |
| `?lang=he` / `?lang=en` | Interface language (not saved) |
| `?theme=light` / `?theme=dark` | Theme (not saved) |
| `?update=<ms>` | Show the "update ready" pill after `<ms>` (default 45 s) |
| `?still=1` | No simulated activity from other members |

In the demo: a message containing `/fail` fails to send (shows retry); the
server "החבר׳ה" has nobody online (messages stay pending); an invite link
containing `offline` finds nobody online.

## Layout

```
src/
  core/        typed core API (commands + events), types, mock implementation
  app/         App root, client-side store, settings (theme/scale/font/locale), age helpers
  i18n/        he.json, en.json – ICU messages; no strings in components
  screens/     onboarding, shell (rail, channels, chat, members, home), settings
  components/  shared bits (Dialog on <dialog>, Avatar, Name/<bdi>, Code, icons)
  styles/      theme.css (CSS variables, light/dark), app.css (logical properties only)
  test/        vitest: mock core, i18n checks, app smoke tests
```

## Rules the code follows

- All text comes from `src/i18n/*.json` (ICU, with plurals). A test fails on
  JSX text, missing ids, or Hebrew/English key drift.
- Hebrew copy is gender-neutral (plural or infinitive) and never says
  seed, key or peer ("מילות שחזור", "הצפנה", "חברי השרת"). A test checks it.
- Messages use `dir="auto"`; names are wrapped in `<bdi>`; recovery words and
  identity codes are always LTR.
- Directional CSS uses logical properties, so the layout mirrors by itself:
  in Hebrew the server rail is on the right, in English on the left.
- The message list is virtualised (TanStack Virtual) and reads messages from
  the core in pages of 100, so a 100k-message channel keeps ~25 rows in the DOM.

## Swapping in the real core

Implement `CoreApi` from `src/core/api.ts` over IPC (e.g. Tauri `invoke` for
commands and `listen` for events) and return it from `createCore()` in
`src/core/index.ts`. Every command is async and takes and returns plain JSON;
messages are read by range (`getMessageCount` + `getMessages(start, end)`), and
edits/deletes are tombstones so message indexes stay stable.
