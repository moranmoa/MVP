# B17 – כיוון טקסט מעורב

- **משימה:** B17 (P0, אבן דרך M1) · **ממצאים:** UX-01, UX-03, UX-18 · **גם מתקן:** U4 (יחידות באחסון)
- **מקורות:** `docs/reviews/ux-review-1.md`, `docs/reviews/po-review-1.md` (§8, B17), החלטת UX-19
- **קבצים צפויים:** `ui/src/app/bidi.ts` (חדש), `MessageList.tsx`, `ChatView.tsx`, `SettingsScreen.tsx`, `components/common.tsx`, `styles/app.css`, `i18n/*.json`, `test/i18n.test.ts`, `test/bidi.test.ts` (חדש)

## 1. מטרה

הודעה בעברית שמתחילה במילה לועזית ("lol", "GG", "@Itamar") מוצגת היום כ-LTR: הסדר נשבר והיא מיושרת רחוק מהכותב. גם מחרוזות ממשק עם שם ערוץ לועזי ("כתיבה ב-#clips") וקיצורי מקלדת ("Ctrl+=") מתהפכות.

אחרי השינוי:
- הכיוון של כל פסקה בהודעה נקבע לפי **רוב** התווים החזקים בה.
- היישור תמיד **לצד הממשק**.
- כל משתנה במחרוזת ממשק מבודד.

## 2. הכלל: `textDir`

פונקציה אחת ב-`ui/src/app/bidi.ts`, ולה משתמשים בכל מקום:

```ts
export type Dir = 'rtl' | 'ltr';
/** Direction of one paragraph: an explicit leading mark wins, then the majority of strong letters, then the UI direction. */
export function textDir(text: string, uiDir: Dir): Dir;
/** Splits on \n and returns each paragraph with its direction. */
export function paragraphs(text: string, uiDir: Dir): { text: string; dir: Dir }[];
```

הסדר:
1. **סימן מפורש:** אם הפסקה מתחילה ב-RLM (U+200F) – `rtl`. ב-LRM (U+200E) – `ltr`. ה-sanitizer כבר משאיר את הסימנים האלה בהודעות, אז זה פתח מילוט למי שרוצה לכפות כיוון.
2. **ספירה:** סופרים אותיות RTL (`\p{Script=Hebrew}`, `\p{Script=Arabic}`, `\p{Script=Syriac}`, `\p{Script=Thaana}`, `\p{Script=Nko}`) ואותיות LTR (כל `\p{L}` אחר). **לא סופרים:** כתובות (`https?://\S+`), אזכורים (`@\S+`), ותוכן בתוך `` `…` ``.
3. **רוב:** יותר RTL – `rtl`. יותר LTR – `ltr`.
4. **שוויון או אפס אותיות** (למשל "🎉🎉" או "3:1") – כיוון הממשק.

ביצועים: הודעה מוגבלת ל-4,000 תווים (PR #10), כך שהספירה זולה. מחשבים פעם אחת לכל `id` + `editedAt` (`useMemo` בשורה).

### טבלת בדיקה (חובה ב-`test/bidi.test.ts`)

| קלט | ממשק | כיוון צפוי |
| --- | --- | --- |
| `lol זה היה מטורף` | he / en | rtl |
| `GG על המשחק אתמול, 3:1 לנו!` | he / en | rtl |
| `@Itamar תבדוק את ה-patch החדש` | he / en | rtl |
| `version 2.4.1 fixed the crash` | he / en | ltr |
| `🎉🎉` | he | rtl |
| `🎉🎉` | en | ltr |
| `https://youtu.be/abcdefg תראו את זה` | he / en | rtl |
| `‏ok` (RLM בהתחלה) | en | rtl |
| שתי פסקאות: `שלום לכולם` ⏎ `npm run build` | he | פסקה 1 rtl, פסקה 2 ltr |

## 3. איפה זה חל

| מקום | היום | אחרי |
| --- | --- | --- |
| גוף הודעה (`.msg-body`) | `<p dir="auto">` אחד | `<p class="msg-body">` ובתוכו `<span class="msg-line" dir={p.dir}>` לכל פסקה |
| ציטוט בתגובה (`.reply-excerpt`) | `dir="auto"` | `dir={textDir(excerpt)}` |
| שורת "תגובה ל-…" / "עריכת הודעה" מעל תיבת הכתיבה | – | אותו כלל על הציטוט |
| תיבת כתיבה | `dir="auto"` | ריקה: `dir={uiDir}`. עם טקסט: `dir={textDir(text, uiDir)}`, מחושב בכל שינוי |
| תצוגה מקדימה בהגדרות ← מראה | `dir="auto"` | אותו רכיב כמו בצ'אט. להוסיף 2 דוגמאות (טבלה ב-§6) |
| שמות (משתמש, שרת, ערוץ) | `<bdi>` | ללא שינוי |

### יישור

כל פסקה מיושרת לצד הממשק, בלי קשר לכיוון שלה:

```css
/* Elements that carry their own dir align to their parent's direction, which is the UI's. */
.msg-line { display: block; text-align: match-parent; unicode-bidi: isolate; }
.reply-excerpt, .composer textarea { text-align: match-parent; }
```

`text-align: match-parent` מיישר לפי הכיוון של ההורה (הממשק), ו-`dir` של השורה קובע רק את סדר המילים. אם יש דפדפן במעטפת שלא תומך – fallback: `:root[dir=rtl] .msg-line { text-align: right } :root[dir=ltr] .msg-line { text-align: left }`.

## 4. מחרוזות ממשק עם משתנים (UX-03, U4)

**כלל:** כל משתנה ב-ICU שהערך שלו **לא** מספר בלבד עטוף ב-FSI (U+2068) ו-PDI (U+2069), בתוך קובץ ה-JSON עצמו: `"⁨{channel}⁩"`.

- משתנים שחייבים עטיפה: `channel`, `name`, `server`, `code`, `version`, `used`, `limit`, `positions`, `age`, וכל משתנה חדש שהוא טקסט.
- פטורים (מספר בלבד): `step`, `total`, `n`, `count`, `size`, ו-`#` בתוך plural.
- **בדיקה ב-`i18n.test.ts`:** לכל מחרוזת ב-he וב-en, כל `{x}` (מחוץ לרשימת הפטורים ולא בתוך `{x, plural…}`) חייב להופיע בדיוק כ-`⁨{x}⁩`. מחרוזת שמפרה את זה מכשילה את הבדיקה.
- `sanitize.ts` מסיר FSI/PDI מטקסט של peers. העטיפה נוספת **בתבנית** ולא בערך, ולכן אין התנגשות. אסור להריץ `sanitize` על פלט של `formatMessage`.

**`settings.storage.used` (U4):** הערכים `used` ו-`limit` מעוצבים עם `Intl.NumberFormat(locale, { style: 'unit', unit: 'megabyte' | 'gigabyte', unitDisplay: 'short' })` ונעטפים ב-FSI/PDI. כך "451 MB" נשאר בסדר הנכון בתוך משפט עברי.

## 5. קיצורי מקלדת (UX-18)

rich text של react-intl עם תגית `<kbd>`:

```tsx
<FormattedMessage id="settings.zoomHint" values={{ kbd: (c) => <kbd dir="ltr">{c}</kbd> }} />
```

```css
kbd { font: inherit; font-size: 0.875em; padding: 0 0.3em; border-radius: 4px; background: var(--bg-input); border: 1px solid var(--border); unicode-bidi: isolate; }
```

אותו דבר ל-`chat.editHint`.

## 6. מחרוזות

dev-ui הוא היחיד שכותב לקבצי ה-JSON. `⁨` ו-`⁩` בטבלה נכתבים בקובץ בדיוק כך, כ-escape של JSON.

| מפתח | עברית | English | שינוי |
| --- | --- | --- | --- |
| `chat.messages` | `הודעות ב-#⁨{channel}⁩` | `Messages in #⁨{channel}⁩` | עטיפה |
| `chat.empty` | `זו תחילת הערוץ #⁨{channel}⁩. אפשר לכתוב את ההודעה הראשונה.` | `This is the start of #⁨{channel}⁩. Write the first message.` | עטיפה |
| `chat.composer` | `כתיבה ב-#⁨{channel}⁩` | `Message #⁨{channel}⁩` | עטיפה |
| `chat.replyingTo` | `תגובה ל-⁨{name}⁩` | `Replying to ⁨{name}⁩` | עטיפה |
| `chat.code` | `קוד זהות ⁨{code}⁩` | `Identity code ⁨{code}⁩` | עטיפה |
| `members.open` | `הפרופיל של ⁨{name}⁩` | `⁨{name}⁩'s profile` | עטיפה |
| `profile.kickConfirm` | `להרחיק את ⁨{name}⁩ מהשרת? …` (שאר הטקסט ללא שינוי) | `Remove ⁨{name}⁩ from the server? …` | עטיפה |
| `join.ageBlocked` | `השרת הזה מיועד לגילאי ⁨{age}⁩ ומעלה, ולכן אי אפשר להצטרף אליו.` | `This server is for ages ⁨{age}⁩ and over, so you can't join it.` | עטיפה |
| `settings.storage.used` | `בשימוש: ⁨{used}⁩ מתוך ⁨{limit}⁩` | `Using ⁨{used}⁩ of ⁨{limit}⁩` | עטיפה + `Intl.NumberFormat` |
| `settings.about.version` | `גרסה ⁨{version}⁩` | `Version ⁨{version}⁩` | עטיפה |
| `restore.invalid` | `{positions}` ← `⁨{positions}⁩` בכל הענפים | כנ"ל | עטיפה |
| `settings.zoomHint` | `קיצורים: <kbd>Ctrl+=</kbd> להגדלה, <kbd>Ctrl+-</kbd> להקטנה, <kbd>Ctrl+0</kbd> לאיפוס.` | `Shortcuts: <kbd>Ctrl+=</kbd> zoom in, <kbd>Ctrl+-</kbd> zoom out, <kbd>Ctrl+0</kbd> reset.` | `<kbd>` |
| `chat.editHint` | `<kbd>Enter</kbd> לשמירה · <kbd>Esc</kbd> לביטול` | `<kbd>Enter</kbd> to save · <kbd>Esc</kbd> to cancel` | `<kbd>` |
| `settings.preview.sample3` (חדש) | `lol זה היה מטורף` | `lol זה היה מטורף` | זהה בשתי השפות – בכוונה |
| `settings.preview.sample4` (חדש) | `GG על המשחק אתמול, 3:1 לנו!` | `GG על המשחק אתמול, 3:1 לנו!` | זהה בשתי השפות – בכוונה |

## 7. כותרת הודעה (UX-19, הוכרע)

נוגעים בשורה הזו בכל מקרה, אז מיישמים כאן את ההכרעה:
- קוד הזהות נשאר ליד השם **בכל הודעה**: `font-size: 0.75rem; color: var(--text-muted); margin-inline-start: 0.375rem`.
- `aria-hidden="true"` על הקוד ועל ראשי התיבות באווטאר.
- ל-`role="article"`: `aria-labelledby="{authorId} {timeId}"`, כך שקורא מסך אומר "הילה, 07:11", ואז את גוף ההודעה.
- ב-hover על הקוד: `title` עם `chat.code` (לא משתנה).

## 8. פריסה

- **RTL (עברית):** הודעות בכל כיוון מיושרות לימין, צמודות לשם ולאווטאר. אנגלית בתוך הודעה עברית נשארת במקום הנכון במשפט.
- **LTR (אנגלית):** הודעות בכל כיוון מיושרות לשמאל. הודעה עברית מיושרת לשמאל, וסדר המילים בתוכה RTL.
- **טלפון (375px):** אותו כלל. אין שינוי פריסה.
- **200% / גופן 24px:** אותו כלל. פסקאות ארוכות נשברות לשורות לפי הכיוון שלהן, וכל השורות מיושרות לאותו צד.

## 9. מצבים

| מצב | התנהגות |
| --- | --- |
| תיבת כתיבה ריקה | `dir` = כיוון הממשק. הטקסט המקדים והסמן בצד הממשק |
| הקלדה | הכיוון מחושב מחדש בכל הקשה. כשהתיבה מתרוקנת – חוזר לכיוון הממשק |
| הודעה מחוקה / חסומה | בלי שינוי (טקסט ממשק) |
| הודעה ארוכה מקוצרת (`clipped`) | הכיוון לפי הטקסט המוצג |
| אין אותיות (אימוג'י, מספרים) | כיוון הממשק |
| הודעה שנערכה | מחשבים מחדש (ה-memo תלוי ב-`editedAt`) |

## 10. מקלדת וקורא מסך

- אין שינוי בסדר הפוקוס.
- `dir` על כל `.msg-line` גורם לקורא המסך לקרוא את המילים בסדר הנכון. לא מוסיפים `lang`, כי שפת ההודעה לא ידועה.
- `<kbd>` מוקרא כטקסט רגיל ("Ctrl+=").
- כותרת ההודעה – לפי §7.

## 11. CoreApi

אין שינוי בחוזה. הכל קורה בממשק.

## 12. בדיקות קבלה

**אוטומטיות:**
1. `bidi.test.ts`: כל השורות בטבלה של §2 עוברות.
2. `i18n.test.ts`: הכלל של §4 נאכף. בדיקה שלילית: מחרוזת עם `{channel}` בלי עטיפה מכשילה.
3. בדיקת רכיב: `.msg-line` של `lol זה היה מטורף` מקבל `dir="rtl"`, ו-`getComputedStyle().textAlign` שלו `right` בעברית ו-`left` באנגלית.

**צילומים** (ל-`screenshots.mjs`; מצב דמו חדש `?demo=1&bidi=1`, שמוסיף לערוץ "כללי" את 9 ההודעות של §2):

| # | צילום | מה רואים |
| --- | --- | --- |
| 70 | `70-bidi-he-dark` | כל 9 ההודעות מיושרות לימין, צמודות לשם. "GG … 3:1 לנו!" עם סימן הקריאה בסוף המשפט (משמאל). "@Itamar" בתחילת המשפט (מימין) |
| 71 | `71-bidi-en-light` | אותן הודעות, כולן מיושרות לשמאל. בהודעות העבריות סדר המילים תקין |
| 72 | `72-composer-he-latin-channel` | ערוץ `clips`, תיבה ריקה: הטקסט המקדים "כתיבה ב-#clips" בסדר הזה, מיושר לימין |
| 73 | `73-composer-he-typing` | הקלדת "GG על המשחק" בתיבה: מיושר לימין וקריא |
| 74 | `74-settings-zoom-hint-he` | "Ctrl+=", "Ctrl+-" ו-"Ctrl+0" במסגרות מקש, בסדר הנכון |
| 75 | `75-settings-storage-he` | "בשימוש: 451 MB מתוך 5 GB" – המספר לפני היחידה (U4) |
| 76 | `76-bidi-he-200` | כמו 70, ב-200% ובגופן 24px, ב-1366×768 |
| 77 | `77-bidi-he-phone` | כמו 70, ב-375px |
| 78 | `78-message-head-code` | קוד הזהות קטן ומעומעם ליד כל שם (UX-19) |

## 13. מחוץ לתחום

- U5 (שמות ספריות בשורת הרישיונות) – B12.
- מפרידי תאריכים – B16.
- כיוון שמות שרתים וערוצים בפס ובכותרות – כבר ב-`<bdi>`, ללא שינוי.
