# Profile save – one save, every place updates at once

- **Source:** owner feedback on 0.4.3 (P1 for 0.4.5): "I changed the profile picture, but it didn't change my picture in the member list or in the conversations. The colour changed only after a restart." And: "The save behaviour isn't right."
- **Builds on:** B11e (the editor), #362 (pictures and colour everywhere). It doesn't redesign the editor.
- **Screenshots:** 400–409.

## 1. What happens today (0.4.3, from the code)

| Place | Source of my picture and colour | After "שמירה" |
| --- | --- | --- |
| My panel (bottom of the channel list) | `identity` | ✅ Updates at once (`IdentityChanged`) |
| Member list, message avatars, profile card, voice roster, mention list, server-settings members | My **`Member` row** in each server | ❌ Stays old. `updateProfile` emits only `IdentityChanged`, not `members.changed`. The row changes only when my profile post comes back through sync, or after a restart |
| Other members | My profile post | ⏳ Up to **10 minutes** later (`PROFILE_POST_EVERY_MS`), and only while I'm online |

So the editor *does* save, but the app looks as if it didn't. That's the owner's "the save behaviour isn't right".

Also:
- **Feedback:** "הפרופיל נשמר" appears next to the button, but nothing in the rest of the app confirms it.
- **What's local and what's shared:** the bio and the full-size picture stay on this device, while the name, colour and a 48×48 picture are shared. Today only the hint text says so.

## 2. Intended behaviour

**One model: an explicit "שמירה"** (the B11e exception, approved). No auto-save for any field, colour and picture included. Picking a colour or a picture changes the **preview card** only, until "שמירה".

**On "שמירה":**
1. The button shows "שומרים…", disabled.
2. **Everywhere on this computer updates in the same frame**: panel, member list, messages (old and new), profile card, voice roster, mention list, and server-settings members. Rule: **any place that shows me renders from `identity`, not from my `Member` row** (UI fix). The core also emits `members.changed` for my row in every server, so the UI doesn't depend on the overlay alone (core fix).
3. The toast "הפרופיל נשמר" (`profile.edit.saved`), with a second line if others will see it later: `profile.edit.savedLater` ("אחרים בשרתים יראו את השינוי בדקות הקרובות.").
4. The editor leaves the dirty state (no dot on "חשבון", and the save bar isn't sticky).

**Others:** the core sends the profile post **right after an explicit save** (the throttle stays for repeated saves: if the last post was under a minute ago, it coalesces). The target is that others see the change within 1 minute while connected. Tech Lead: `PROFILE_POST_EVERY_MS` stays as an upper bound for bursts, but a user-initiated save should flush promptly.

**Errors:** `profile.edit.saveError` under the buttons, with the draft kept and "שמירה" enabled again. Nothing changes anywhere else (no half-applied state).

**Unsaved changes when leaving** (unchanged from B11e, confirmed): switching settings pages or closing the dialog with a draft shows the existing confirmation (`unsavedTitle`, with "שמירה" / "ביטול השינויים" / "חזרה לעריכה").

```
 Before save                         After save (same frame)
 ┌ Settings → Account ─────┐         panel      ● new picture
 │ [preview: new picture]  │         members    ● new picture
 │ צבע  ● ● ✓● ●           │   ──►   messages   ● new picture (old messages too)
 │ [שמירה] [ביטול שינויים] │         voice      ● new picture
 └─────────────────────────┘         toast: "הפרופיל נשמר · אחרים יראו בדקות הקרובות"
```

## 3. Strings

| Key | Hebrew | English |
| --- | --- | --- |
| `profile.edit.saved` | `הפרופיל נשמר` (unchanged) | `Profile saved` (unchanged) |
| `profile.edit.savedLater` | `אחרים בשרתים יראו את השינוי בדקות הקרובות.` | `Others in your servers will see it within a few minutes.` |
| `profile.edit.saving` | `שומרים…` (unchanged) | `Saving…` (unchanged) |

## 4. Acceptance criteria

1. Change the picture → "שמירה" → my avatar is new in the panel, member list, my old messages, a new message, the profile card, the voice roster (while in a room), and mention suggestions. No restart needed.
2. Same for the colour (no picture): the letter-on-colour is new in every place above.
3. Same for the name.
4. Before "שמירה", only the preview card changes. Every other place still shows the old picture, colour and name.
5. With `?profileError=1` (mock): the error shows, the draft stays, and no other place changes.
6. Leaving with a draft shows the unsaved confirmation. "ביטול השינויים" restores the saved values in the preview.
7. Core test: `updateProfile` emits `members.changed` for my row in each server where I'm a member.
8. Two computers (manual or e2e): after a save on A, B sees the new picture and colour within 1 minute while both are online.

**Screenshots:**

| # | Shot | What it shows |
| --- | --- | --- |
| 400 | `400-profile-saved-everywhere-he` | After saving a new colour: the panel, member list and messages all show it, with the toast |
| 401 | `401-profile-draft-preview-only-he` | A draft colour: only the preview card has changed |
