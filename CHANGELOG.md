# Changelog

What changed in each MVS release, newest first. The release workflow puts
the section of the tagged version into the release notes, and a tag build
fails if its version has no section here. Write a section before tagging:
`## [X.Y.Z] - YYYY-MM-DD`, then Hebrew and English (Fixed / New / Known).
Never put the relay domain or any IP address here.

## [0.2.1] - 2026-11-04

### מה תוקן
- **הצטרפות לשרת:** משתמש חדש שמצטרף בלינק יכול לכתוב מיד (תוך כ-15 שניות), בלי לחכות שבעל השרת ישלח הודעה. בגרסה 0.2.0 המצטרף קיבל "אין הרשאה" עד שהבעלים כתב.
- **העתקת לינק הזמנה:** כפתור ההעתקה מעתיק באמת, ומציג "הועתק" רק כשההעתקה הצליחה. הלינק מוצג גם כטקסט שאפשר לסמן ולהעתיק ידנית (Ctrl+C). חדש: פאנל הזמנה גם בסרגל הצד.

### מה חדש
- **מפתח עדכונים:** האפליקציה כוללת את המפתח הציבורי לאימות עדכונים חתומים. עדכון אוטומטי בתוך האפליקציה יופעל בגרסה הבאה; מ-0.2.0 ל-0.2.1 מתקינים ידנית מעל הגרסה הקיימת.
- **שרת ה-relay:** תיקון בצד השרת למגבלת חיבורים שחלקו בה כל המשתמשים (לא דורש התקנה).

### ידוע
- אין עדיין יצירת ערוצים נוספים, הגדרות שרת ותפריט קליק ימני (מתוכנן). אין עדיין קול.
- הבנייה לא חתומה בתעודה: ב-Windows בחרו "More info" ← "Run anyway".

---

### Fixed
- **Joining a server:** a new member who joins by link can write right away (within about 15 s), without waiting for the owner to post. In 0.2.0 they got "not allowed" until the owner wrote.
- **Copying the invite link:** the copy button really copies, and shows "Copied" only when it worked. The link is also shown as selectable text (Ctrl+C), and there's a new invite panel in the sidebar.

### New
- **Update key:** the app embeds the public key used to verify signed updates. In-app auto-update arrives in the next version; 0.2.0 → 0.2.1 is a manual install over the old one.
- **Relay:** a server-side fix for a connection limit that every user shared (no install needed).

### Known
- No channel creation, server settings or right-click menus yet (planned). No voice yet.
- Builds aren't code-signed: on Windows choose "More info" → "Run anyway".

## [0.2.0] - 2026-11-04

### מה חדש
- גרסת האלפא הסגורה הראשונה: מתקין ל-Windows (למשתמש, בלי הרשאות מנהל), AppImage ו-‎.deb ל-Linux.
- יצירת זהות עם 12 מילות שחזור, יצירת שרת, הזמנה בלינק `mvs://` וצ'אט דרך שרת ה-relay של הפרויקט.

### ידוע
- פורמט הנתונים עוד יכול להשתנות: שרתים והודעות מהגרסה הזאת עשויים להתאפס בגרסה מאוחרת יותר. הזהות נשמרת עם 12 המילים.
- מי שהצטרף בלינק לא יכול היה לכתוב עד שבעל השרת כתב (תוקן ב-0.2.1).
- הבנייה לא חתומה בתעודה: ב-Windows בחרו "More info" ← "Run anyway".

---

### New
- First closed-alpha pre-release: a Windows installer (per user, no admin), and an AppImage and `.deb` for Linux.
- Create an identity with 12 recovery words, create a server, invite by `mvs://` link, and chat through the project's relay.

### Known
- The data format may still change: servers and messages made with this build may be reset by a later one. Your identity is safe as long as you keep your 12 recovery words.
- A member who joined by link couldn't write until the owner had posted (fixed in 0.2.1).
- Builds aren't code-signed: on Windows choose "More info" → "Run anyway".
