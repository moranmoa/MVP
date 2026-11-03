# B24 – מסך "אודות"

- **משימה:** B24 · **מקורות:** `docs/business/plan-1.md` §3.1 (תנאי שימוש), §3.2 (הודעת פרטיות), §3.3 (דיווח על פגיעה), B9 (קובץ third-party notices), U5 (שורת הרישיונות)
- **קבצים צפויים:** `SettingsScreen.tsx` (אודות), `legal/he.json` ו-`legal/en.json` (חדשים), `Onboarding.tsx` (ההסכם קורא מאותו מקור), `core/api.ts`, `i18n/*.json`

## 1. מבנה העמוד ("הגדרות ← אודות")

מלמעלה למטה:

1. **כותרת המוצר:** "MVS" (`.brand`, כמו במסך הראשון של ההרשמה), ומתחת `settings.about.version`, עם כפתור קטן "העתקה" (`about.copyVersion`). הוא מעתיק `MVS {version} ({os})` לדיווחי באגים, ומציג "הועתק".
2. `settings.about.alpha` (ללא שינוי).
3. **ערוץ עדכונים** – החלק הקיים, ללא שינוי.
4. **"מסמכים"** (`about.legal`): שתי שורות-קישור, כל אחת פותחת תצוגה בתוך ההגדרות (§2):
   - "תנאי השימוש" (`about.terms`)
   - "הודעת הפרטיות" (`about.privacy`)
5. **"דיווח על פגיעה או תוכן לא חוקי"** (`about.report`):
   - הסבר: `about.report.body`
   - הכתובת: `abuseContact` מ-`getAppInfo()` (§4), ב-`Code`, עם "העתקה" ו"שליחת מייל" (פותח `mailto:` דרך `openExternal`)
   - **בגרסת dev או דמה בלבד:** כשהשדה ריק, במקום הכתובת מופיע `about.report.pending`, בלי כפתורים. בגרסה שמופצת לבודקים הכתובת תמיד קיימת, כי הבנייה נכשלת בלעדיה (§4)
6. **"פרטיות – פניות"**: אותו דפוס, עם `privacyContact` (מ-`getAppInfo()`) ו-`about.privacyContact.body`. ריק – `about.report.pending`.
7. **"רישיונות קוד פתוח"** (`about.licenses`): שורת-קישור שפותחת את רשימת הרכיבים (§3). מחליפה את `settings.about.licenses`, וכך גם נפתר U5.

## 2. תצוגת מסמך (תנאים, פרטיות)

- בתוך הפאנל של ההגדרות, עם כפתור "חזרה" (חץ בצד ה-start) וכותרת המסמך.
- **המקור:** `legal/{he,en}.json`, מבנה `{ terms: { updated, sections: [{ title, body }] }, privacy: { … } }`. מרנדרים ל-`<h3>` ו-`<p>`, **בלי markdown ובלי HTML** (CSP, ‏PR #10).
- **אותו מקור לשלב ההסכם בהרשמה:** שם מוצגות רק כותרות הסעיפים (7 + סעיף 8 על העדכונים, לפי plan-1 §3.1), כמו היום. "אודות" מציג את הגוף המלא.
- בראש המסמך: `about.doc.updated` ("עודכן ב-{date}").
- **placeholders בטיוטה** (`[כתובת דיווח]`, `[קישור]`): מוחלפים בזמן ההצגה ב-`abuseContact` ובקישור הפנימי לתצוגת הפרטיות. ערך ריק מוצג כ-`about.report.pending`. בתצוגה לא מופיעים סוגריים מרובעים.
- טקסט שמסומן "[לבדיקת עו"ד: …]" ב-plan-1 **לא** נכנס לאפליקציה.

## 3. רישיונות קוד פתוח

- רשימה מתוך קובץ ה-third-party notices שנוצר ב-B9 (JSON בבנייה: `name`, `version`, `license`).
- בכל שורה: השם (`<bdi dir="ltr">`), הגרסה, והרישיון. לחיצה פותחת את טקסט הרישיון באותו דפוס כמו §2.
- בראש: `about.licenses.intro`.
- עד ש-B9 קיים: הרשימה מ-`package-lock.json` (הסקריפט `npm run licenses` כבר קורא אותו).

## 4. CoreApi

| פקודה | שינוי | הערה |
| --- | --- | --- |
| `openExternal(url): Promise<void>` | **חדש** | פותח `mailto:` או `https:` מחוץ לאפליקציה, דרך המעטפת (Tauri opener או Electron `shell`). הליבה מקבלת רק את שתי הסכמות האלה. במעטפת חסומים ניווט וחלונות חדשים (PR #10), ולכן אין `window.open` |
| `getAppInfo()` | **שינוי חוזה:** מוסיפים `os: string` (למשל "Windows 11"), `abuseContact?: string` ו-`privacyContact?: string` | `os` לכפתור "העתקה". שתי הכתובות מגיעות מקונפיגורציית השחרור בזמן הבנייה (`ABUSE_CONTACT`, `PRIVACY_CONTACT`), **ולא נכנסות ל-git**. בניית release נכשלת אם `ABUSE_CONTACT` ריק – כתובת הדיווח היא חלק מהמינימום של האלפא הסגורה (plan-1 §3.3) |

## 5. מצבים

| מצב | מה רואים |
| --- | --- |
| כתובת דיווח ריקה (dev או דמה בלבד) | `about.report.pending`, בלי כפתורים |
| `openExternal` נכשל | toast `about.openFailed`. הכתובת נשארת להעתקה |
| מסמך ארוך | גלילה רגילה בתוך הפאנל. כפתור "חזרה" sticky למעלה |
| 200% / גופן 24px | שורות-הקישור בגובה 44px לפחות. שמות רכיבים ארוכים נשברים (`overflow-wrap: anywhere`) |
| טלפון 375px | ההגדרות במסך מלא (PR #10). תצוגת המסמך מחליפה את העמוד, ו"חזרה" חוזר ל"אודות" |
| אופליין | הכל מקומי. "שליחת מייל" פותח את תוכנת המייל, והיא מטפלת בחיבור |

## 6. מקלדת וקורא מסך

- **שורות-הקישור:** `<button>` עם חץ (`aria-hidden`). Enter פותח.
- **פתיחת מסמך:** הפוקוס עובר לכותרת שלו (`tabIndex=-1`). "חזרה" או Esc (כשאין דיאלוג פתוח מעליו) מחזירים לשורה שנלחצה.
- **"העתקה":** ההודעה "הועתק" ב-`aria-live="polite"`.
- **הכתובת:** `dir="ltr"` בתוך `<bdi>`.

## 7. מחרוזות

| מפתח | עברית | English |
| --- | --- | --- |
| `about.copyVersion` | `העתקת פרטי הגרסה` | `Copy version details` |
| `about.legal` | `מסמכים` | `Documents` |
| `about.terms` | `תנאי השימוש` | `Terms of use` |
| `about.privacy` | `הודעת הפרטיות` | `Privacy notice` |
| `about.doc.updated` | `עודכן ב-⁨{date}⁩` | `Updated ⁨{date}⁩` |
| `about.report` | `דיווח על פגיעה או תוכן לא חוקי` | `Report abuse or illegal content` |
| `about.report.body` | `אין שרת מרכזי, ולכן אנחנו לא יכולים למחוק תוכן מהמחשבים של אחרים. על הודעה בשרת כדאי לדווח קודם לבעלי השרת. על תוכן לא חוקי, ובפרט פגיעה בקטינים, אפשר לדווח גם לנו:` | `There's no central server, so we can't delete content from other people's computers. Report a message to the server's owner first. Illegal content, especially anything harming minors, can also be reported to us:` |
| `about.report.email` | `שליחת מייל` | `Send email` |
| `about.report.pending` | `הכתובת תופיע בגרסה שמופצת לבודקים.` | `The address appears in builds shipped to testers.` |
| `about.privacyContact` | `פניות בנושא פרטיות` | `Privacy requests` |
| `about.privacyContact.body` | `לשאלות, ולבקשות לעיון במידע או למחיקה:` | `For questions, and requests to see or delete data:` |
| `about.licenses` | `רישיונות קוד פתוח` | `Open-source licenses` |
| `about.licenses.intro` | `MVS בנויה על רכיבי קוד פתוח. תודה לכל מי שכתבו אותם.` | `MVS is built on open-source components. Thanks to everyone who wrote them.` |
| `about.openFailed` | `לא הצלחנו לפתוח את תוכנת המייל. אפשר להעתיק את הכתובת.` | `Couldn't open your email app. Copy the address instead.` |
| `settings.about.licenses` | **יורד** (מוחלף ב-`about.licenses` ובתצוגה של §3) | – |

הערה: "MVS בנויה" – MVS מתפקד כשם עצם נקבה ("האפליקציה"), כמו ב-plan-1. הוא מופיע בתחילת משפט, בלי תחילית עברית (ההערה מ-PR #10 על כיווניות).

## 8. בדיקות קבלה

1. **dev בלי כתובת:** `about.report.pending` מוצג, ואין כפתור "שליחת מייל" ב-DOM.
1a. **release:** בנייה עם `ABUSE_CONTACT` ריק נכשלת (בדיקה ב-CI של B9). אין כתובת דיווח או פרטיות בקבצים שב-git (בדיקת מקור).
2. **עם כתובת (דמה: `?demo=1&abuse=1`):** "שליחת מייל" קורא ל-`openExternal('mailto:…')`.
3. **"תנאי השימוש"** נפתח עם הכותרת בפוקוס. אין `[` או `]` בטקסט שמוצג.
4. **אותו מקור:** שלב ההסכם בהרשמה ו"תנאי השימוש" באודות מציגים את אותן כותרות סעיפים, באותו סדר.
5. **U5:** ברשימת הרישיונות כל שם רכיב מוצג LTR ושלם ("TanStack Virtual").

**צילומים:**

| # | צילום | מה רואים |
| --- | --- | --- |
| 140 | `140-about-he` | הדף המלא, עם ה-placeholder של כתובת הדיווח |
| 141 | `141-about-terms-he` | תצוגת תנאי השימוש |
| 142 | `142-about-licenses-en-light` | רשימת הרישיונות באנגלית |
| 143 | `143-about-phone-he` | 375px |
