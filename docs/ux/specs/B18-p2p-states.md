# B18 – מצבי P2P בממשק

- **משימה:** B18 (P1) · **ממצאים:** UX-07, UX-08, UX-09, QA-08
- **ניסוחים מהאפיון (לא משתנים):** "ממתינה – תישלח כשמישהו מהשרת יתחבר", "אף אחד מהשרת לא מחובר כרגע. אפשר לקרוא את מה שכבר שמור אצלך ולכתוב – ההודעות יישלחו כשמישהו יתחבר", "מחפשים חברים מהשרת…"
- **קבצים צפויים:** `ServerRail.tsx`, `ChatView.tsx`, `MessageList.tsx`, `Home.tsx` (דיאלוג ההצטרפות ומסך "השרת מוכן"), `components/PendingJoin.tsx` (חדש), `core/api.ts`, `core/types.ts`, `core/mock.ts`, `i18n/*.json`

## 1. מטרה

המשתמש צריך לראות בלי להיכנס לשרת:
- באיזה שרת אין אף אחד מחובר;
- כמה הודעות שלו ממתינות;
- איזו הצטרפות עדיין מחפשת ברקע.

בשרת שבו רק אני, מציגים הזמנה לחברים ולא אזהרה.

## 2. מצב השרת

שלושה מצבים לכל שרת, מהליבה (§7):

| מצב | מתי | בפס השרתים | בערוץ |
| --- | --- | --- | --- |
| `connecting` | אחרי הפעלה, עד שהליבה יודעת (עד ~10 שניות) | כרגיל, בלי סימון | בלי פס. כך אין הבהוב של "אף אחד לא מחובר" בכל הפעלה |
| `online` | לפחות מחשב אחד נוסף מהשרת מחובר | כרגיל | כרגיל |
| `nobody-online` | אף מחשב אחר לא מחובר | טבעת מקווקוות (§3) | הפס הקיים `chat.offline`, או מצב "רק אני" (§4) |

## 3. פס השרתים

**שרת `nobody-online`:**
- טבעת מקווקוות של 2px בצבע `--offline` סביב האייקון, במרחק 3px. האייקון עצמו ב-`opacity: 0.7`.
- שם נגיש ו-`title`: `rail.serverNobodyOnline` ("{server} · אף אחד לא מחובר").

**הודעות ממתינות** (`pendingCount > 0`):
- תג בפינה התחתונה בצד ה-end: אייקון שעון + מספר, רקע `--bg-elevated`, טקסט `--text`, גבול `--border`.
- אם יש גם אזכורים – התג האדום של האזכורים מנצח, ותג ההמתנה עובר לפינה העליונה בצד ה-end.
- נוסף לשם הנגיש: `rail.pendingCount`.

**הצטרפות ממתינה (QA-08):**
- לכל הצטרפות שממשיכה ברקע: עיגול זמני בפס, **מתחת לשרתים ומעל "+"**. גבול מקווקו, spinner קטן במרכז, ו-`title` = `join.pending.title`.
- לחיצה פותחת popover מעוגן לעיגול (בטלפון: דיאלוג במסך מלא):
  - כותרת `join.pending.title`;
  - `join.searching` (מהאפיון) + `join.pending.body`;
  - הלינק ב-`Code`, בשורה אחת עם `…`;
  - "הפסקת החיפוש" (`join.pending.cancel`, משני מסוכן) – קורא ל-`cancelJoin(link)`, מוריד את העיגול ומציג toast `join.pending.cancelled`;
  - "סגירה" (`common.close`).
- **כשנמצא חבר מחובר:** ההצטרפות עוברת לשלב התצוגה המקדימה של ש-23 (B11a–d §3.3). העיגול מקבל את אייקון השרת, עם הטבעת המקווקוות ונקודה כחולה. toast: `join.preview.found` עם "לפרטים".
- **אחרי אישור (`confirmJoin`):** העיגול הופך לאייקון השרת במקומו, עם אנימציית fade של 150ms (בלי אנימציה תחת `prefers-reduced-motion`). toast: `join.joinedServer`. לא עוברים לשרת אוטומטית.
- **כשהצטרפות נכשלת ברקע** (`invalid`, `age-blocked`, `revoked` מ-B11d): העיגול יורד, ו-toast עם הסיבה (המחרוזות הקיימות `join.invalid`, `join.ageBlocked`, ו-`join.revoked` מ-B11d) ו"סגירה".
- כמה הצטרפויות – כמה עיגולים, לפי הסדר שבו התחילו.

## 4. שרת שבו רק אני (UX-07)

**תנאי:** `memberCount === 1` (רק אני).

- **לא מציגים** את הפס `chat.offline`.
- במקום `chat.empty`, במרכז אזור ההודעות, כרטיס:
  - אייקון של שני אנשים;
  - כותרת `server.solo.title`;
  - טקסט `server.solo.body`;
  - כפתור ראשי "העתקת לינק ההזמנה" (`server.invite`). אחרי לחיצה: "הועתק ✓" ל-2 שניות, עם `aria-live`.
- **תיבת הכתיבה נשארת פעילה.** הודעות שנכתבות כאן ממתינות כרגיל, ומופיעות מעל הכרטיס.
- כשמצטרף חבר ראשון: הכרטיס יורד, ומצב השרת נקבע לפי §2.

**מסך "השרת מוכן"** (בסוף יצירת שרת): מתחת ללינק נוספת השורה `create.done.availability`.

## 5. בתוך הערוץ

- **פס `chat.offline`** – הניסוח מהאפיון, ללא שינוי. כשהליבה יודעת מתי מישהו היה מחובר לאחרונה (`lastSeenAt`), נוספת מתחתיו שורה משנית: `chat.offline.lastSeen`, עם `Intl.RelativeTimeFormat` ("לפני שעתיים").
- **הודעה ממתינה:** "ממתינה – תישלח כשמישהו מהשרת יתחבר" (מהאפיון), ולצידה כפתור קישור "ביטול" (`chat.pending.cancel`). הוא מוחק את ההודעה מהתור המקומי, אחרי אישור קצר בתוך השורה ("לבטל את השליחה?" עם "ביטול השליחה" / "השארה").
- **בסרגל הפעולות של הודעה ממתינה:** "עריכה" נשאר (עורך את הטקסט שבתור). "מחיקה" מוחלף ב"ביטול השליחה".

## 6. מצבים

| מצב | מה רואים |
| --- | --- |
| הפעלה (`connecting`) | בלי טבעות ובלי פס offline. תגי המתנה כן מוצגים, כי הם מקומיים |
| שרת `nobody-online` | טבעת בפס, פס offline בערוץ |
| רק אני | כרטיס ההזמנה, בלי פס offline ובלי טבעת – הטבעת מיועדת לשרתים שהיו בהם אנשים |
| הצטרפות ממתינה | עיגול עם spinner בפס |
| הצטרפות הושלמה / נכשלה | §3 |
| ביטול הודעה ממתינה נכשל (נשלחה בינתיים) | השורה מתעדכנת ל"נשלחה", ו-toast `chat.pending.alreadySent` |
| שם שרת ארוך (48 תווים) ב-200% / 24px | ב-`title` ובשם הנגיש של האייקון – השם המלא. ב-popover של הצטרפות ממתינה יש רק לינק, ולא שם. ב-toast השם נחתך עם `…` אחרי 32 תווים |
| טלפון 375px | הטבעת והתגים כמו בדסקטופ (הפס קיים בעמוד הערוצים). popover ההצטרפות הוא דיאלוג במסך מלא. כרטיס "רק אני" ברוחב מלא |

## 7. CoreApi

| פקודה / אירוע / טיפוס | שינוי | הערה |
| --- | --- | --- |
| `ServerSummary` | **שינוי חוזה:** מוסיפים `reachability: 'connecting' \| 'online' \| 'nobody-online'`, `memberCount: number`, `pendingCount: number`, `lastSeenAt?: Timestamp` | `pendingCount` = ההודעות שלי במצב `pending` בכל ערוצי השרת. `lastSeenAt` = הפעם האחרונה שמחשב אחר מהשרת היה מחובר |
| `server.updated` | ללא שינוי בחתימה | נשלח כשאחד השדות החדשים משתנה |
| `listPendingJoins(): Promise<PendingJoin[]>` | **חדש** | `PendingJoin = { link: string; startedAt: Timestamp }` |
| `join.pending.changed` | **אירוע חדש:** `{ joins: PendingJoin[] }` | נשלח בכל הוספה או הסרה |
| `join.status` | ללא שינוי | `joined`, `invalid`, `age-blocked` ו-`revoked` (B11d) מגיעים גם להצטרפות ברקע |
| `cancelJoin(link)` | ללא שינוי | מסיר מהרשימה ושולח `join.pending.changed` |
| `cancelPendingMessage(messageId): Promise<void>` | **חדש** | נדחה עם `{ code: 'not-allowed' }` אם ההודעה כבר לא `pending` |
| `editMessage` | ללא שינוי | עובד גם על הודעה `pending` (מעדכן את התור) |
| דמה | `?demo=1` מוסיף שרת `nobody-online` עם 2 הודעות ממתינות, שרת "רק אני", והצטרפות ממתינה ללינק עם `offline`, שמסתיימת אחרי 20 שניות | לצילומים |

## 8. מקלדת וקורא מסך

- העיגול של הצטרפות ממתינה הוא חלק מה-roving tabindex של הפס (חצים), כמו כל שרת. Enter או רווח פותחים את ה-popover. Esc סוגר ומחזיר את הפוקוס לעיגול.
- ה-popover: `role="dialog"` עם `aria-labelledby`. הפוקוס על "הפסקת החיפוש".
- שינויי מצב (שרת עבר ל-`nobody-online`, הצטרפות הושלמה) **לא** מוקראים בכל פעם, כדי לא להציף. ה-toasts הם `role="status"`.
- כרטיס "רק אני": הכותרת `h2`. הכפתור הוא הפריט הראשון אחרי הכותרת בסדר ה-Tab.
- תג ההמתנה הוא `aria-hidden`. המידע עובר בשם הנגיש של כפתור השרת.

## 9. מחרוזות

| מפתח | עברית | English | שינוי |
| --- | --- | --- | --- |
| `rail.serverNobodyOnline` | `⁨{server}⁩ · אף אחד לא מחובר` | `⁨{server}⁩ · nobody online` | חדש |
| `rail.pendingCount` | `{count, plural, one {הודעה אחת ממתינה} two {שתי הודעות ממתינות} other {# הודעות ממתינות}}` | `{count, plural, one {# message waiting} other {# messages waiting}}` | חדש |
| `join.pending.title` | `מחפשים את השרת…` | `Looking for the server…` | חדש |
| `join.pending.body` | `נמשיך לחפש ברקע. השרת יופיע ברשימה ברגע שמישהו ממנו יתחבר.` | `We'll keep looking in the background. The server appears as soon as someone from it connects.` | חדש |
| `join.pending.cancel` | `הפסקת החיפוש` | `Stop looking` | חדש |
| `join.pending.cancelled` | `החיפוש הופסק.` | `Stopped looking.` | חדש |
| `join.joinedServer` | `ההצטרפות ל-⁨{server}⁩ הצליחה` | `You joined ⁨{server}⁩` | חדש |
| `server.solo.title` | `השרת מוכן. עכשיו צריך חברים.` | `Your server is ready. Now invite friends.` | חדש |
| `server.solo.body` | `אין שרת מרכזי: השרת קיים על המחשבים של החברים בו. כשיצטרפו עוד אנשים, הוא יישאר זמין גם כשהמחשב הזה כבוי.` | `There's no central server: it lives on its members' computers. Once more people join, it stays available even when this computer is off.` | חדש |
| `create.done.availability` | `השרת זמין כשלפחות מחשב אחד מהשרת מחובר.` | `The server is available while at least one member's computer is online.` | חדש |
| `chat.offline.lastSeen` | `מישהו מהשרת היה מחובר לאחרונה ⁨{time}⁩.` | `Someone from the server was last online ⁨{time}⁩.` | חדש. `time` מ-`RelativeTimeFormat` |
| `chat.pending.cancel` | `ביטול` | `Cancel` | חדש |
| `chat.pending.cancelConfirm` | `לבטל את השליחה?` | `Cancel sending?` | חדש |
| `chat.pending.cancelYes` | `ביטול השליחה` | `Cancel sending` | חדש |
| `chat.pending.keep` | `השארה` | `Keep` | חדש |
| `chat.pending.alreadySent` | `ההודעה כבר נשלחה.` | `The message was already sent.` | חדש |
| `chat.offline`, `chat.state.pending`, `join.searching`, `join.nobody`, `join.keepTrying` | ללא שינוי (מהאפיון) | ללא שינוי | – |

## 10. בדיקות קבלה

**אוטומטיות:**
1. **QA-08:** הצטרפות עם לינק `offline` ← "להמשיך לחפש ברקע" ← עיגול בפס עם `title` = "מחפשים את השרת…". כשהדמה מוצאת חבר – toast "השרת … נמצא". "לפרטים" ← "הצטרפות" ← העיגול הופך לשרת. בדיקת QA-08 ב-`explore.spec.ts` עוברת מ-`test.fail()` לירוק.
2. "הפסקת החיפוש" ← `cancelJoin` נקרא והעיגול יורד.
3. השם הנגיש של כפתור שרת `nobody-online` עם 2 ממתינות: "החבר׳ה · אף אחד לא מחובר, שתי הודעות ממתינות".
4. שרת חדש מיד אחרי היצירה: אין `.offline-banner` ב-DOM, ויש כרטיס עם "העתקת לינק ההזמנה".
5. ב-`connecting` אין טבעת ואין פס offline.
6. ביטול הודעה ממתינה ← היא נעלמת והמונה בפס יורד באחד.

**צילומים:**

| # | צילום | מה רואים |
| --- | --- | --- |
| 100 | `100-rail-nobody-online-he` | "החבר׳ה" עם טבעת מקווקוות ותג שעון "2" |
| 101 | `101-rail-pending-join-he` | עיגול עם spinner מתחת לשרתים, ו-popover פתוח עם "הפסקת החיפוש" |
| 102 | `102-solo-server-he` | שרת חדש: כרטיס "השרת מוכן. עכשיו צריך חברים." במרכז, בלי פס צהוב |
| 103 | `103-offline-last-seen-he` | הפס הצהוב עם שורת "היה מחובר לאחרונה לפני שעתיים" |
| 104 | `104-pending-cancel-he` | הודעה ממתינה עם "ביטול", ואישור בתוך השורה |
| 105 | `105-rail-states-en-light` | 100 + 101 באנגלית, LTR (הטבעת והתג בצד הנכון) |
| 106 | `106-p2p-200` | 200% + 24px: הטבעת והתגים לא נחתכים |
| 107 | `107-pending-join-phone-he` | 375px: popover ההצטרפות כדיאלוג במסך מלא |
