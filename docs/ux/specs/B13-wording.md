# B13 – ניסוח ושם המוצר: טבלת מחרוזות

- **משימה:** B13 · **מקורות:** הכרעה 27 (MVS), ש-1, ש-2, ש-5, ש-8, ש-9 (`docs/reviews/po-review-1.md` §7), UX-39 ו-UX-40 (`docs/reviews/ux-review-1.md` §4)
- **חלוקה:** העדכונים ב-`docs/spec.md` – product-lead. כאן: מה משתנה בממשק, וטבלת המחרוזות ל-dev-ui (היחיד שכותב ל-`ui/src/i18n/*.json`).
- **קבצים צפויים:** `i18n/*.json`, `core/mock.ts` ו-`Home.tsx` (לינקים), `Onboarding.tsx` ו-`Restore.tsx` (ש-8), `core/api.ts`

## 1. שם המוצר ולינקים

| מקום | מצב היום | נדרש |
| --- | --- | --- |
| `app.name`, `document.title` | MVS | ללא שינוי |
| Electron: `productName`, סכמת הלינקים (`desktop/src/deeplink.js`) | MVS, `mvs` | ללא שינוי |
| לינקי הזמנה בדמה (`core/mock.ts`, 2 מקומות) | `mvp://join/…` | `mvs://join/…` |
| placeholder בשדה "הצטרפות" (`Home.tsx`) | `mvp://join/…` | `mvs://join/…` |
| הבדיקה ב-`security.test.tsx` | `mvp://` | `mvs://` |
| קישור שמודבק עם `mvp://` | – | לא מתקבל. השגיאה הרגילה של לינק לא תקין. אין לינקים כאלה מחוץ לדמה |

- **"MVP" נשאר** במסמכים כשמדובר בשלב הפיתוח ("ה-MVP הוא אלפא סגורה"), ולא מופיע בממשק.
- **במשפט בעברית:** MVS בלי תחילית צמודה ("של MVS", "בגרסה של MVS"), כמו בהערה של B24 §7. המין הדקדוקי: נקבה ("MVS בנויה").

## 2. ההכרעות בממשק

| הכרעה | מה משתנה בממשק | מחרוזות |
| --- | --- | --- |
| **ש-1** "נמסרה" | ✓✓ נקרא "נמסרה", עם הסבר ב-`title` | `chat.state.received`, `chat.state.receivedHint` – ב-B6 §7 |
| **ש-2** "קוד זהות" | אין "מפתח" או "key" בממשק. הקוד ליד כל שם נקרא "קוד זהות" (כבר כך). UX-19 – ליד השם, 0.75rem, `--text-muted` | בדיקה בלבד (§4, בדיקה 3) |
| **ש-5** ביטול גרסה | אין מסך חדש. החזרה האוטומטית לא חוזרת לגרסה שבוטלה. אם המעדכן מסמן שהגרסה הנוכחית בוטלה, הבאנר של העדכון משתמש ב-`update.revoked` במקום `update.ready` | `update.revoked` – רק אם dev-updater חושף את הסימון. אחרת, נשאר באפיון בלבד |
| **ש-8** תאריך לידה בשחזור | שחזור חשבון שואל שוב מדינה ותאריך לידה, ומציג את ההסכם (§3) | `restore.*` ב-§5 |
| **ש-9** `ts` רחוק | מסנן מקומי בלבד, בלי ממשק | – |

## 3. שחזור חשבון (ש-8)

**היום:** "שחזור" בשלב השפה פותח ישר את 12 השדות, ואחרי השחזור מגיעים לאפליקציה. אין הסכם, מדינה או תאריך לידה.

**נדרש (כפי שנבנה ב-#293):** שפה ← "שחזור" ← הסכם ← מדינה ← תאריך לידה ← 12 המילים ← פרופיל ← הודעת "החשבון שוחזר" ← האפליקציה. פרטי הגיל באים לפני המילים, כי `restoreIdentity` מקבל אותם באותה קריאה (#290). כך מי שמתחת לגיל 16 נחסמים לפני שהם מקלידים את המילים.

- **מונה:** "שלב x מתוך 6" (שפה, הסכם, מדינה, תאריך לידה, מילים, פרופיל). זה רצף נפרד מהרצף של הרשמה חדשה.
- **ההסכם:** אותו שלב כמו בהרשמה, עם אותם סעיפים (B24 §2).
- **המילים:** המסך הקיים, עם הבדיקה הקיימת (`restore.invalid`). "שחזור" קורא ל-`restoreIdentity` עם כל הנתונים. בסוף: הודעת `onb.restored.*` (השרתים לא חוזרים אוטומטית).
- **תאריך הלידה:** השלב הקיים, ומעליו `restore.dob.hint`. מתחת לגיל 16 – מסך החסימה הרגיל, ולא נשמר כלום.
- **כלל 30 הימים (הכרעה 18) מול טענת גיל קודמת:** אחרי השחזור והסנכרון, אם לזהות כבר יש בשרת כלשהו טענת גיל חתומה מוקדמת יותר, והתאריך החדש נותן גיל **גבוה** ממנה, זה נחשב תיקון: הטענה החדשה נכנסת לתוקף אחרי 30 יום, ועד אז חלה הקודמת. גיל **נמוך** יותר חל מיד. אין מסך חדש: מופיע הסימון הרגיל של תיקון ממתין (`age.dob.pending`, B7 §2). בלי טענה קודמת, התאריך חל מיד, כמו בחשבון חדש.
- **בסוף:** קריאה אחת ל-`restoreIdentity` עם כל הנתונים. "משחזרים…" (`restore.working`) על הכפתור. כישלון – ההודעה הרגילה, והנתונים נשארים בשדות.
- **"חזרה" בכל שלב** חוזר לשלב הקודם עם הערכים שכבר הוזנו, כולל המילים (בזיכרון בלבד).
- **המשך אחרי reload:** לא נשמר. reload באמצע השחזור חוזר לשלב השפה. המילים לא נשמרות בשום מקום.

**CoreApi:**

| פקודה | שינוי | הערה |
| --- | --- | --- |
| `restoreIdentity(words, profile: { locale, country, birthDate })` | **שינוי חוזה** (היום: `restoreIdentity(words)`) | אותם כללי גיל כמו `createIdentity`. דוחה עם `too-young` מתחת ל-16, ועם `invalid-words` |
| `restoreIdentity`: כלל 30 הימים מול טענה קודמת, אחרי הסנכרון | **שינוי חוזה** (הכרעה 18) | אם יש טענת גיל חתומה קודמת והגיל החדש גבוה ממנה: `Identity.pendingBirthDate` (B7 §9) עם `effectiveAt` = הסנכרון + 30 יום, ועד אז הטענה הקודמת חלה. גיל נמוך יותר – מיד |

## 4. בדיקות קבלה

1. **שם:** אין `mvp://` ב-`ui/src` (חוץ מבדיקה שמוודאת שלינק כזה נדחה). אין "MVP" במחרוזות.
2. **ש-1:** אין "התקבלה אצל חבר" או "Delivered to a member" בקבצי המחרוזות.
3. **ש-2:** ב-`he.json` אין "מפתח". ב-`en.json` אין "key" (חוץ מ-"keyboard"). לבדוק ב-grep ב-CI.
4. **ש-8:** שחזור עובר דרך הסכם, מילים, מדינה ותאריך לידה, ו-`restoreIdentity` מקבל את שלושתם. תאריך מתחת לגיל 16 – מסך החסימה, ו-`restoreIdentity` לא נקרא.
4a. **שחזור לא עוקף את 30 הימים (דמה):** זהות עם טענת גיל 17 בשרת עם ערוץ 18+ ← שחזור עם תאריך שנותן 18 ← אחרי הסנכרון יש `pendingBirthDate` עם `effectiveAt` = היום + 30 יום, `age.dob.pending` מוצג, והערוץ עדיין מוסתר. שחזור עם תאריך שנותן גיל נמוך מהטענה חל מיד.
5. **ניסוח (§5):** כל מפתח בטבלה מכיל את הטקסט החדש, בשתי השפות.
6. **`restore.invalid`:** `positions` עטוף ב-FSI/PDI (B17 §4), ומעוצב עם `Intl.ListFormat` כמו ב-`onb.check.wrong`.

**צילומים:**

| # | צילום | מה רואים |
| --- | --- | --- |
| 160 | `160-restore-agreement-he` | ההסכם בתוך רצף השחזור, "שלב 2 מתוך 5" |
| 161 | `161-restore-dob-he` | תאריך הלידה עם `restore.dob.hint` |
| 162 | `162-members-online-he` | רשימת החברים: "אונליין – 3" / "אופליין – 2" |
| 163 | `163-home-en-light` | Home באנגלית עם "in the sidebar" |
| 164 | `164-join-placeholder-he` | שדה ההצטרפות עם `mvs://join/…` |

## 5. טבלת המחרוזות

**מ-UX-39 ו-UX-40 (עוד לא בוצעו):**

| מפתח | עברית | English | הערה |
| --- | --- | --- | --- |
| `chat.blocked` | `הודעה מחשבון חסום` | `Message from a blocked account` | מגדר (UX-39) |
| `members.online` | `אונליין – {count}` | `Online – {count}` | אותה מילה כמו בכרטיס הפרופיל |
| `members.offline` | `אופליין – {count}` | `Offline – {count}` | כנ"ל |
| `onb.preparing.done` | `הכול מוכן` | `All set` | כתיב מלא |
| `onb.agreement.item7` | `יש להזין תאריך לידה נכון. מתחת לגיל 16 אין להשתמש באפליקציה, ואין להצטרף לשרתים עם הגבלת גיל גבוהה יותר.` | `Enter your real date of birth. You must be 16 or older to use the app, and you can't join servers with a higher age limit.` | |
| `home.subtitleWithServers` | `השרתים שלך בפס שבצד. אפשר גם ליצור שרת חדש או להצטרף לעוד אחד.` | `Your servers are in the sidebar. You can also create a new one or join another.` | |
| `restore.invalid` | `{count, plural, one {המילה במקום ⁨{positions}⁩ לא מוכרת} two {המילים במקומות ⁨{positions}⁩ לא מוכרות} other {המילים במקומות ⁨{positions}⁩ לא מוכרות}}. כדאי לבדוק את האיות.` | `{count, plural, one {Word ⁨{positions}⁩ isn't recognized} other {Words ⁨{positions}⁩ aren't recognized}}. Check the spelling.` | FSI (B17), אנגלית אמריקאית |
| `chat.toggleMembers` | **יורד**, ובמקומו שני מפתחות: | | עם `aria-pressed` |
| `chat.showMembers` | `הצגת החברים` | `Show members` | חדש |
| `chat.hideMembers` | `הסתרת החברים` | `Hide members` | חדש |
| `profile.kickConfirm` | ללא שינוי | `Remove ⁨{name}⁩ from the server? The server's encryption will be updated so this account can't read new messages.` | אנגלית בלבד |
| `settings.about.licenses` | **יורד** (B24 §7) | | |

**חדשות (§2, §3):**

| מפתח | עברית | English | הערה |
| --- | --- | --- | --- |
| `restore.dob.hint` | `תאריך הלידה לא נשמר במילות השחזור, ולכן צריך להזין אותו שוב.` | `Your date of birth isn't part of the recovery words, so enter it again.` | ש-8 |
| `update.revoked` | `נמצאה תקלה בגרסה הזו. גרסה מתוקנת מוכנה – הפעלה מחדש` | `This version has a problem. A fixed version is ready – restart` | ש-5, רק אם יש סימון |

**נשארים כמו שהם (לא לשנות):** `onb.words.warning`, `join.searching`, `notice.encryptionRenewed`, `chat.state.pending`, `chat.offline`, וכל מחרוזת בבנייה "מי ש…" או ברבים ("חברי השרת", "ברוכים הבאים").
