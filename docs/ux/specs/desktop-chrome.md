# Desktop chrome – logo, title bar, minimize

- **Source:** owner requests (2026-11-11). This covers three short items. **Screenshots:** 390–399.
- **Files expected:** `desktop/src/main.js` (window options), `desktop/src/preload.js` (one window-control command), `ui/src/components/TitleBar.tsx` (new), `desktop/build/` (icons).

## 1. App logo

**Concept** ([`../assets/mvs-logo.svg`](../assets/mvs-logo.svg)): a speech bubble whose body is three connected dots, so a chat with no central server, just peers talking to each other. It's simple enough to read at 16px, where the bubble and three dots survive and the connecting lines may blur, and that's fine.

| Use | Form |
| --- | --- |
| App icon, installer, taskbar, about screen (256, 128, 64, 48, 32 px) | The tile: rounded square in `#4752c4` (the light-theme `--accent`), white bubble, indigo peers |
| 16 px (window title bar, tab, favicon) | The same tile, with the lines dropped and the dots enlarged (a separate 16 px SVG, hand-tuned) |
| On the accent colour, or in single-colour contexts (notifications on Linux, print) | Monochrome: the bubble and peers in `currentColor`, no tile |
| Dark theme | The same tile. The indigo tile reads on both light and dark backgrounds. Don't swap it to `#7b86f0` |

**Rules:**
- Keep clear space of at least ⅛ of the tile on every side. Don't add a drop shadow, outline or gradient.
- Don't recolour the tile per server or theme. Don't rotate or stretch it.
- The wordmark is "MVS" in the app's UI font, semibold, set beside the mark (never inside it). It isn't used at 16px.
- Exports for the build: `icon.ico` (16/24/32/48/64/256) for Windows, PNGs at 16–512 for Linux (`.desktop`), and later `icon.icns` (macOS, R8).

The owner should approve the concept before the icons are exported. Alternatives are cheap at this stage.

## 2. Custom title bar

**Goal:** the app's own top bar instead of the OS frame, so the window looks like one product. It keeps every behaviour people expect from a window.

```
 ┌──────────────────────────────────────────────────────────────────┐
 │ [mark] MVS · גיימרים ישראל                       │ –  │ ▢ │ ✕ │  ← 32px
 └──────────────────────────────────────────────────────────────────┘
   └ drag area: everything except the logo menu and the buttons ┘
```

- **Height:** 32px. Background: `--bg-rail` (the same as the server rail), so it joins the left column visually.
- **Content (start side):** the 16px mark and "MVS". After a "·", the current server's name (one line, ellipsis, `<bdi>`). No other controls: the update pill and notices stay where they are.
- **Drag area:** the whole bar except the buttons (`-webkit-app-region: drag`, with `no-drag` on interactive parts). Double-click maximizes or restores, as the OS does.
- **Window controls:**
  - **Windows:** use Electron's `titleBarStyle: 'hidden'` with `titleBarOverlay` (the native minimize, maximize and close buttons, coloured to match the theme). Snap Layouts on hover over maximize, and Alt+Space, keep working. Buttons are 46×32, as native.
  - **Linux:** `frame: false`, with our own three buttons (minimize, maximize/restore, close). They're 32×32, with the same icons on all desktops, and a red hover only on close. One IPC command `window(action)` with `action` ∈ `minimize | toggle-maximize | close`. Nothing else goes through the bridge (GAP-17 stays minimal).
- **RTL:** the window controls stay on the **physical right**, as the OS puts them, in both languages. Only the bar's content (mark, name) follows the UI direction: on the right in Hebrew (before the controls) and on the left in English.
- **Accessibility:** the Linux buttons have `aria-label`s (`window.minimize` / `window.maximize` / `window.restore` / `window.close`) and are reachable with Tab. A keyboard user can also use the OS shortcuts (Super+↑ and so on). The bar is a `role="banner"` landmark.
- **Theme:** follows light and dark at once, including the Windows overlay colours (`setTitleBarOverlay` on theme change).
- **Fullscreen:** the bar hides.

## 3. Minimize goes to the taskbar

- **Minimize** (button, or Win+↓) puts the window on the taskbar, as normal. **There's no tray icon** in v1.
- **Close (✕)** quits the app. If I'm in a call: a confirmation, "השיחה תסתיים. לסגור את MVS?" with "סגירה" and "ביטול".
- **Notifications** (G-61) arrive while the app is minimized, because it's still running. After close, nothing arrives until the next start. The settings hint already says notifications appear "כשהחלון של MVS לא בחזית", and that stays true.
- **Unread count on the taskbar icon** (G-62): `setOverlayIcon` on Windows, and the badge count on Linux where the desktop supports it.

## 4. Strings

| Key | Hebrew | English |
| --- | --- | --- |
| `window.minimize` | `מזעור` | `Minimize` |
| `window.maximize` | `הגדלה` | `Maximize` |
| `window.restore` | `שחזור גודל` | `Restore` |
| `window.close` | `סגירה` | `Close` |
| `window.closeInCall` | `השיחה תסתיים. לסגור את MVS?` | `This will end your call. Close MVS?` |

## 5. Acceptance criteria

1. Windows: the window has no OS frame, and the native buttons sit on the overlay at the top right. Snap Layouts appear on hover over maximize.
2. Linux: three buttons at the top right. Minimize, maximize/restore and close work, and nothing other than `window(action)` is exposed to the page.
3. Dragging anywhere on the bar except the buttons moves the window. Double-click maximizes and restores.
4. In Hebrew the mark and server name sit on the right of the bar, and the window buttons stay on the physical right.
5. Theme switch: the bar and the Windows overlay colours change at once.
6. Minimize shows the app on the taskbar, with no tray icon. Close quits. During a call, close asks first.
7. The logo reads at 16px in the title bar (screenshot 390) and at 256px in the installer.

**Screenshots:**

| # | Shot | What it shows |
| --- | --- | --- |
| 390 | `390-title-bar-he-light` | The bar in Hebrew, light |
| 391 | `391-title-bar-en-dark` | The bar in English, dark |
| 392 | `392-close-in-call-he` | The confirmation on close during a call |
