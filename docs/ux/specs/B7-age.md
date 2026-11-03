# B7 – גיל בממשק

- **משימה:** B7 (P0, אבן דרך M2) · **ממצאים:** UX-15, UX-23, QA-04 · **החלטות:** פרק "גיל והגבלות גיל" באפיון, ש-8 (תאריך בשחזור), ש-12 (ערוץ 18+ מוסתר), BOARD 5.10 (אלפא סגורה – ישראל בלבד, ממתין לבעלים)
- **תלוי ב:** הליבה (טענות גיל חתומות – core-5), B11b (העלאת הגבלה), B11c (סימון 18+), B11e (עמוד החשבון)
- **קבצים צפויים:** `Onboarding.tsx` (מדינה, תאריך לידה), `SettingsScreen.tsx` (חשבון), `Home.tsx` (יצירת שרת, הודעות), `Shell.tsx`, `app/age.ts`, `core/api.ts`, `core/types.ts`, `core/mock.ts`, `i18n/*.json`

## 1. מה נכלל

1. תיקון תאריך לידה בהגדרות, עם כלל 30 הימים.
2. בדיקת 120 שנה (QA-04), בהרשמה ובתיקון.
3. ערוצי 18+ מוסתרים מבני 16–17. הממשק מטפל במצב שבו ערוץ נעלם.
4. הודעה לחברים שאיבדו גישה כשהגבלת שרת עלתה, או אחרי תיקון שהוריד את הגיל.
5. ניסוח אפשרויות הגיל ביצירת שרת (UX-15).
6. ברירת מחדל למדינה, ו"למה צריך?" (UX-23).

## 2. תיקון תאריך לידה – "הגדרות ← חשבון"

מתחת לפרופיל (B11e) ומעל מילות השחזור, קטע **"תאריך לידה"**:
- שורה: התאריך הנוכחי (`Intl.DateTimeFormat`, ‏`dateStyle: 'long'`) + כפתור משני "תיקון" (`age.dob.edit`).
- מתחת: `age.dob.privacy` (טקסט משני). זה אותו רעיון כמו `onb.dob.hint`: התאריך נשאר במכשיר.

**לחיצה על "תיקון"** פותחת דיאלוג (`age.dob.editTitle`):
1. אותם שלושה שדות כמו בהרשמה (יום, חודש, שנה), מלאים בתאריך הנוכחי, עם התיקונים של UX-24.
2. אחרי שהתאריך תקין, מתחת לשדות מופיע **תיאור התוצאה**. הוא מגיע מ-`previewBirthDateCorrection` (§7):

| התוצאה | מה מוצג | כפתור ראשי |
| --- | --- | --- |
| בלי שינוי בגיל (למשל אותה שנה, יום אחר) | `age.dob.effect.same` | "שמירה" |
| **הגיל יורד** | `age.dob.effect.lowerNow`. אם יש שרתים או ערוצים שהגישה אליהם תיסגר – `age.dob.effect.lowerLoses` ורשימה של השמות (עד 5, ו"ועוד {count}") | "שמירה" (מסוכן, אם הרשימה לא ריקה) |
| **הגיל עולה** | `age.dob.effect.raiseLater`, עם התאריך: היום + 30 יום | "שמירה" |
| **מתחת ל-16** | `age.dob.effect.tooYoung`. אין כפתור שמירה | – |
| **לפני יותר מ-120 שנה** | §3 | – |

3. אחרי "שמירה":
   - **ירידה:** חלה מייד. השרתים והערוצים יורדים, ומופיעה הודעה לפי §5.
   - **עלייה:** נשמרת כ**תיקון ממתין**. בקטע מתחת לתאריך: `age.dob.pending` וכפתור קישור "ביטול התיקון" (`age.dob.cancelPending`).
4. יכול להיות רק תיקון ממתין אחד. תיקון חדש מחליף את הקודם, והדיאלוג מציין את זה: `age.dob.replacesPending`.

**מתחת ל-16 – לאישור מנהל המוצר:** האפיון לא מגדיר מה קורה כשמתקנים לתאריך מתחת לגיל המינימום. ההצעה: הממשק לא מאפשר לשמור, ומציג `age.dob.effect.tooYoung`. הליבה דוחה עם `too-young`. כך לא נוצר חשבון "נעול" בלי מסלול יציאה. החלופה, שבה החשבון נחסם במכשיר, דורשת מסך חסימה ומדיניות מחיקה.

**שחזור (ש-8):** בשחזור מבקשים שוב את תאריך הלידה, באותו מסך כמו בהרשמה (שלב חדש ב-`RestoreScreen`, B2). כלל 30 הימים לא חל שם. מעל השדות: `age.restore.hint`.

## 3. בדיקת 120 שנה (QA-04)

בהרשמה ובתיקון, אם השנה מוקדמת ביותר מ-120 שנה מהיום:
- מתחת לשדות, במקום תיאור התוצאה: `age.dob.veryOld`, עם שני כפתורים: "כן, זה נכון" (`age.dob.veryOldYes`, משני) ו"תיקון התאריך" (`age.dob.veryOldFix`, ראשי, מחזיר את הפוקוס לשדה השנה).
- "כן, זה נכון" ממשיך כרגיל.
- שנה לפני 1900 נשארת `onb.dob.invalid` (כמו היום, ב-`toBirthDate`).

## 4. ערוצי 18+ (ש-12)

- לחבר בלי טענת 18+ הליבה לא שולחת ערוצים שסומנו 18+. **בממשק אין מצב "נעול"**: הערוץ פשוט לא קיים ברשימה. אין תג, ואין רמז שיש ערוץ מוסתר.
- **ערוץ קיים שסומן 18+** בזמן שחבר בן 16–17 נמצא בו: הליבה שולחת `channel.removed` עם `reason: 'age'` ← הממשק עובר ל-#general, ומציג toast `age.channelHidden`. הטקסט בתיבת הכתיבה נשאר (כמו ב-B11c §4.3).
- **כשחבר מגיע לגיל 18** (לפי התאריך, או כשתיקון ממתין נכנס לתוקף): הערוצים מופיעים ברשימה עם נקודת "חדש" רגילה, בלי הודעה נפרדת.
- ‏`channel.adult` (התגית `18+` ליד שם הערוץ) נשארת אצל מי שרואים את הערוץ.

## 5. הודעה על איבוד גישה

**מתי:**
- (א) הבעלים העלו את הגבלת השרת מעל הגיל שלי (B11b);
- (ב) תיקנתי את התאריך כלפי מטה;
- (ג) ערוץ שהייתי בו סומן 18+ – זה ה-toast של §4, בלי כרטיס.

**איך:**
- **אם האפליקציה פתוחה:** toast `age.serverLost` (שרת) ← השרת יורד מהפס. אם הוא היה פתוח – עוברים ל"בית".
- **תמיד (גם אם האפליקציה הייתה סגורה):** כרטיס הודעה בראש "בית", שנשאר עד שסוגרים אותו:
  - אייקון מגן, `age.serverLost.title`
  - טקסט `age.serverLost.body`, עם שם השרת ב-`<bdi>` והגיל
  - כפתור "סגירה" (`common.close`)
  - כמה הודעות – כמה כרטיסים, הכי חדשים למעלה. אחרי 3 – "ועוד {count}", שפותח את כולם.
- **ההודעות נשמרות** ב-`localStorage` (`mvs.notices.v1`: מזהה, סוג, שם שרת, גיל, תאריך – בלי תוכן הודעות), עד שסוגרים אותן.
- **הטקסט לא מאשים** ולא מציע לשקר לגבי הגיל.

## 6. יצירת שרת (UX-15) ומדינה (UX-23)

**אפשרויות הגיל** – הערכים נשארים `none` / `16+` / `18+`. רק הניסוח משתנה, ומתחת לכל אפשרות יש שורת הסבר:

| ערך | תווית | הסבר |
| --- | --- | --- |
| `none` | `create.age.none` | `create.age.noneHint` |
| `16+` | `create.age.16` | – |
| `18+` | `create.age.18` | `create.age.18Hint` |

`create.age.hint` נשאר. אותן אפשרויות גם ב"הגדרות השרת" (B11b §3.1).

**מדינה (UX-23):**
- **ברירת מחדל:** האזור מ-`navigator.languages`. אם אין אזור **וגם** הממשק בעברית – ישראל (כמו היום). **אם הבעלים יאשרו את 5.10** (אלפא בישראל בלבד): ישראל תמיד בברירת מחדל, וזה לא משנה את הלוגיקה – רק את הערך ההתחלתי.
- **ברשימה:** ישראל ראשונה, ואחריה מפריד (`<hr>` בתוך ה-`select`, או `optgroup` "כל המדינות" – `age.country.all`).
- **מתחת לשדה:** `age.country.why`.
- חיפוש ברשימה – B22, לא כאן.

## 7. מצבים

| מצב | מה רואים |
| --- | --- |
| טוען תיאור תוצאה (`previewBirthDateCorrection`) | שורה עם spinner במקום התיאור, ו"שמירה" מושבת |
| שומר | "שמירה" ← "שומרים…" |
| שגיאה בשמירה | `error.generic` עם "לנסות שוב". התאריך הקודם נשאר |
| תיקון ממתין | §2.3 |
| אופליין | התיקון חל מקומית. הליבה שולחת טענת גיל חדשה כשמישהו מחובר. בירידה – השרתים יורדים מהמחשב הזה מייד |
| שם שרת ארוך (48 תווים) בכרטיס ההודעה, ב-200% / 24px | שם עד 2 שורות, ו-`…` אחריהן. הכרטיס ברוחב מלא של "בית" |
| טלפון 375px | דיאלוג התיקון במסך מלא, עם הכפתורים בתחתית. כרטיסי ההודעה ברוחב מלא |

## 8. מקלדת וקורא מסך

- **דיאלוג התיקון:** הפוקוס על שדה היום. תיאור התוצאה הוא `role="status"` (`aria-live="polite"`), ומוקרא אחרי שהקלדה נעצרת ל-500ms.
- **בדיקת 120 שנה:** `role="alert"`. הפוקוס עובר ל"תיקון התאריך".
- **כרטיסי ההודעה:** `role="region"` עם `aria-label` = `age.serverLost.title`. "סגירה" עם `aria-label` = `age.serverLost.closeFor` ("סגירת ההודעה על {server}").
- **toasts:** `role="status"`.

## 9. CoreApi

| פקודה / אירוע / טיפוס | שינוי | הערה |
| --- | --- | --- |
| `Identity` | **שינוי חוזה:** מוסיפים `pendingBirthDate?: { date: BirthDate; effectiveAt: Timestamp }` | |
| `previewBirthDateCorrection(date): Promise<AgeCorrectionPreview>` | **חדש** | `{ kind: 'same' \| 'lower' \| 'raise' \| 'too-young'; effectiveAt?: Timestamp; loses: { serverId; name; channel?: string }[] }`. לא משנה כלום |
| `correctBirthDate(date): Promise<AgeCorrectionPreview>` | **חדש** | מחיל. נדחה עם `{ code: 'too-young' }` (קוד חדש) |
| `cancelBirthDateCorrection(): Promise<void>` | **חדש** | |
| `restoreIdentity(words, birthDate)` | **שינוי חוזה:** מוסיפים `birthDate` | ש-8, עם B2 |
| `server.removed` | **שינוי חוזה:** `reason` מקבל גם `'age'`, ומוסיפים `required?: AgeRestriction` | הרחבה של B11a–d §8 |
| `channel.removed` | **שינוי חוזה:** מוסיפים `reason?: 'deleted' \| 'age'` | הרחבה של B11a–d §8 |
| `identity.changed` | ללא שינוי | גם כשתיקון ממתין נכנס לתוקף |
| דמה | `?demo=1&ageLost=1` יוצר הודעת "השרת מיועד עכשיו ל-18+". `?demo=1&age=17` – משתמש בן 17 (בלי ערוץ "אחרי-חצות") | לצילומים |

## 10. מחרוזות

| מפתח | עברית | English |
| --- | --- | --- |
| `age.dob.section` | `תאריך לידה` | `Date of birth` |
| `age.dob.edit` | `תיקון` | `Correct` |
| `age.dob.editTitle` | `תיקון תאריך הלידה` | `Correct your date of birth` |
| `age.dob.privacy` | `התאריך נשמר מוצפן רק במחשב הזה. שרתים מקבלים רק אישור גיל.` | `Stored encrypted on this computer only. Servers only get an age confirmation.` |
| `age.dob.effect.same` | `הגיל לא משתנה.` | `Your age doesn't change.` |
| `age.dob.effect.lowerNow` | `הגיל יתעדכן מייד.` | `Your age updates right away.` |
| `age.dob.effect.lowerLoses` | `בגיל הזה הגישה לשרתים ולערוצים האלה תיסגר:` | `At this age you'll lose access to:` |
| `age.dob.effect.raiseLater` | `תיקון שמעלה את הגיל ייכנס לתוקף ב-⁨{date}⁩. עד אז נשאר הגיל הנוכחי.` | `A correction that raises your age takes effect on ⁨{date}⁩. Until then your current age stays.` |
| `age.dob.effect.tooYoung` | `התאריך הזה מתחת לגיל 16. השימוש באפליקציה מותר רק מגיל 16, ולכן אי אפשר לשמור אותו.` | `This date is under 16. MVS is for ages 16 and up, so it can't be saved.` |
| `age.dob.pending` | `תיקון ממתין: ⁨{newDate}⁩. ייכנס לתוקף ב-⁨{date}⁩.` | `Pending correction: ⁨{newDate}⁩, from ⁨{date}⁩.` |
| `age.dob.cancelPending` | `ביטול התיקון` | `Cancel correction` |
| `age.dob.replacesPending` | `התיקון הזה יחליף את התיקון שממתין.` | `This replaces the pending correction.` |
| `age.dob.veryOld` | `התאריך הזה לפני יותר מ-120 שנה. זה נכון?` | `That's more than 120 years ago. Is it right?` |
| `age.dob.veryOldYes` | `כן, זה נכון` | `Yes, it's right` |
| `age.dob.veryOldFix` | `תיקון התאריך` | `Fix the date` |
| `age.restore.hint` | `תאריך הלידה לא נשמר במילות השחזור, ולכן צריך להזין אותו שוב.` | `Your date of birth isn't part of the recovery words, so enter it again.` |
| `age.channelHidden` | `הערוץ #⁨{channel}⁩ מיועד עכשיו לגילאי 18 ומעלה.` | `#⁨{channel}⁩ is now for ages 18 and over.` |
| `age.serverLost` | `הגישה לשרת ⁨{server}⁩ נסגרה.` | `Access to ⁨{server}⁩ was closed.` |
| `age.serverLost.title` | `עדכון גישה` | `Access update` |
| `age.serverLost.body` | `השרת ⁨{server}⁩ מיועד לגילאי ⁨{age}⁩ ומעלה, ולכן הגישה אליו נסגרה במחשב הזה.` | `⁨{server}⁩ is for ages ⁨{age}⁩ and over, so access to it was closed on this computer.` |
| `age.serverLost.closeFor` | `סגירת ההודעה על ⁨{server}⁩` | `Dismiss the notice about ⁨{server}⁩` |
| `create.age.none` | `פתוח לכל הגילים שמותרים באפליקציה` | `Open to every age the app allows` |
| `create.age.noneHint` | `היום זה 16 ומעלה. אם האפליקציה תיפתח לגילים צעירים יותר, הם יוכלו להצטרף.` | `Today that's 16 and over. If the app opens to younger ages, they can join.` |
| `create.age.16` | `16 ומעלה – תמיד` | `16 and over – always` |
| `create.age.18` | `18 ומעלה` | `18 and over` |
| `create.age.18Hint` | `מי שמתחת ל-18 לא יראו את השרת בחיפוש ולא יוכלו להצטרף.` | `People under 18 can't find or join the server.` |
| `age.country.all` | `כל המדינות` | `All countries` |
| `age.country.why` | `המדינה קובעת את גיל המינימום לפי החוק בה. היא נשמרת רק במחשב הזה.` | `Your country sets the minimum age under its law. It's stored on this computer only.` |

הערות על המחרוזות:
- `date` ו-`newDate` מעוצבים עם `Intl.DateTimeFormat` לפי שפת הממשק.
- `age` הוא "16" או "18". הוא עטוף ב-FSI, כמו ב-B17.
- `create.age.18Hint` – "מי שמתחת ל-18" הוא סתמי ("מי ש…"), ולכן מותר.

## 11. בדיקות קבלה

**אוטומטיות:**
1. **QA-04:** שנה 1890 בהרשמה ← `age.dob.veryOld`. "כן, זה נכון" ממשיך, ו"תיקון התאריך" מחזיר לשדה השנה. בדיקת QA-04 ב-`explore.spec.ts` עוברת לירוק.
2. **עלייה:** תיקון שמעלה את הגיל ← `pendingBirthDate` עם `effectiveAt` = היום + 30 יום. "ביטול התיקון" מנקה אותו.
3. **ירידה מתחת להגבלה של שרת 18+** ← השרת יורד מהפס, וכרטיס ההודעה מופיע ב"בית". אחרי טעינה מחדש הכרטיס עדיין שם. אחרי "סגירה" – לא.
4. **מתחת ל-16** ← אין כפתור שמירה, ו-`correctBirthDate` לא נקרא.
5. **בן 17 (דמה):** אין "אחרי-חצות" ברשימה. סימון ערוץ פתוח כ-18+ ← מעבר ל-#general ו-toast.
6. **UX-15:** ביצירת שרת מוצגים הניסוחים והשורות מתחת לאפשרויות.

**צילומים:**

| # | צילום | מה רואים |
| --- | --- | --- |
| 130 | `130-account-dob-he` | הקטע "תאריך לידה" בחשבון, עם תיקון ממתין ו"ביטול התיקון" |
| 131 | `131-dob-correct-lower-he` | דיאלוג תיקון: "הגיל יתעדכן מייד", ורשימת שרת אחד שהגישה אליו תיסגר |
| 132 | `132-dob-correct-raise-he` | "ייכנס לתוקף ב-…" |
| 133 | `133-dob-very-old-he` | בדיקת 120 שנה |
| 134 | `134-home-age-notice-he` | כרטיס "עדכון גישה" בראש "בית" |
| 135 | `135-create-age-options-he` | אפשרויות הגיל בניסוח החדש |
| 136 | `136-country-why-he` | ישראל ראשונה, ושורת "המדינה קובעת…" |
| 137 | `137-dob-correct-en-light` | 131 באנגלית |
| 138 | `138-dob-correct-phone-he` | 375px: הדיאלוג במסך מלא |
| 139 | `139-age-notice-200` | 200% + 24px: כרטיס ההודעה עם שם שרת של 48 תווים |
