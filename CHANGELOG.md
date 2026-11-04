# Changelog

What changed in each MVS release, newest first. The release workflow puts
the section of the tagged version into the release notes, and a tag build
fails if its version has no section here. Write a section before tagging:
`## [X.Y.Z] - YYYY-MM-DD`, then Hebrew and English (Fixed / New / Known).
Never put the relay domain or any IP address here.

## [0.2.3] - 2026-11-04

### מה חדש
- **פורט האזנה קבוע:** בהפעלה הראשונה האפליקציה בוחרת פורט ושומרת אותו גם אחרי הפעלה מחדש. כך אפשר לאשר אותו פעם אחת ב-Windows Firewall או בראוטר.
- **ניסיון חיבור ישיר משני הצדדים:** אם החיבור הישיר מהצד הראשון נכשל, הצד השני מנסה בעצמו. מספיק שאחד משני המחשבים יקבל חיבורים נכנסים.

### מה תוקן
- אין תיקונים גלויים מעבר לשיפורי החיבור שלמעלה.

### ידוע
- עדיין אין "ניקוב" חיבור דרך NAT בין רשתות שונות (hole punching); זה הבא בתור. עד אז, בין ראוטרים שונים החיבור עובר דרך ה-relay.
- עוד אין באפליקציה סימון אם החיבור ישיר או דרך ה-relay.
- העדכון האוטומטי בתוך האפליקציה עוד לא פעיל: מתקינים את הגרסה החדשה ידנית מעל הקיימת.

---

### New
- **A stable listening port:** the app picks a port on first start and keeps it across restarts, so you can allow it once in Windows Firewall or on your router.
- **Direct connection tried from both sides:** if the first side's direct connection fails, the other side tries too. Only one of the two computers needs to accept incoming connections.

### Fixed
- Nothing user-visible beyond the connection improvements above.

### Known
- No NAT hole punching between different networks yet; that's next. Until then, computers behind different routers connect through the relay.
- The app doesn't show yet whether a connection is direct or through the relay.
- In-app auto-update isn't active yet: install the new version manually over the old one.

## [0.2.2] - 2026-11-04

### מה חדש
- **חיבור ישיר באותה רשת ביתית:** חברים שמחוברים לאותה רשת (אותו ראוטר) מדברים ישירות זה עם זה, בלי לעבור דרך שרת ה-relay. זה מהיר יותר ופחות תלוי בשרת.
- **מסך "הליבה לא זמינה":** אם חלק הרקע של האפליקציה לא עולה, מופיע מסך עם **לנסות שוב** ו**פתיחת הלוגים**. הלוג מסתיר כתובות רשת.
- **Linux בלי מחזיק מפתחות (keyring):** הודעת שגיאה ברורה שמסבירה מה חסר, במקום כשל לא מובן.

### מה תוקן
- **פרטיות:** כתובות הרשת הביתית נשלחות רק לחברי השרת, ולא לכל מי שמחובר.
- **חיבור:** כששני חיבורים דרך ה-relay נפתחו בו-זמנית, הם חסמו את החיבור הישיר. תוקן.

### ידוע
- עדיין אין חיבור ישיר בין רשתות שונות (ראוטרים שונים); שם ממשיכים דרך ה-relay. זה הבא בתור.
- עוד אין באפליקציה סימון אם החיבור ישיר או דרך ה-relay.
- העדכון האוטומטי בתוך האפליקציה עוד לא פעיל: מתקינים את הגרסה החדשה ידנית מעל הקיימת. הנתונים והזהות נשמרים.

---

### New
- **Direct connection on the same home network:** members on the same network (behind the same router) talk to each other directly instead of through the relay. That's faster and depends less on the server.
- **"Core unavailable" screen:** if the app's background part doesn't start, a screen offers **Try again** and **Open logs**. The log hides network addresses.
- **Linux without a keyring:** a clear error that says what's missing, instead of an unexplained failure.

### Fixed
- **Privacy:** home-network addresses are sent only to the server's members, not to everyone who's connected.
- **Connection:** two relay connections opened at the same time blocked the direct one. Fixed.

### Known
- No direct connection between different networks (different routers) yet; those still go through the relay. That's next.
- The app doesn't show yet whether a connection is direct or through the relay.
- In-app auto-update isn't active yet: install the new version manually over the old one. Your data and identity are kept.

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
