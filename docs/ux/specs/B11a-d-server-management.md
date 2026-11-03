# B11a–d – ניהול שרת: עזיבה, עריכה וסגירה, ערוצים, לינק הזמנה

- **משימות:** B11a (עזיבה), B11b (עריכה וסגירה), B11c (ערוצים), B11d (ביטול לינק) – כולן P1
- **החלטות:** ש-19 (בעלים לא עוזבים), ש-20 (מחיקת ערוץ, #general), ש-21 (ביטול לינק), ש-22 (אייקון), ש-23 (תיאור), ש-10 (ב-MVP רק הבעלים מנהלים), ש-12 (ערוץ 18+ מוסתר מבני 16–17)
- **קבצים צפויים:** `ChannelSidebar.tsx`, `ServerMenu.tsx` (חדש), `ServerSettings.tsx` (חדש), `ChannelDialog.tsx` (חדש), `Home.tsx` (דיאלוג ההצטרפות), `Shell.tsx`, `app/image.ts` (מ-B11e), `core/api.ts`, `core/types.ts`, `core/mock.ts`, `i18n/*.json`

## 1. נקודת הכניסה: תפריט השרת

היום ליד שם השרת יש רק כפתור "העתקת לינק ההזמנה". **שם השרת בראש פס הערוצים הופך לכפתור** עם חץ למטה (כמו בדיסקורד), שפותח את התפריט:

| פריט | מי רואה | פעולה |
| --- | --- | --- |
| **כותרת:** אייקון, שם, תג גיל, והתיאור (עד 3 שורות, `line-clamp`) | כולם | – (ש-23: כל החברים רואים את התיאור) |
| העתקת לינק ההזמנה | כולם | כמו היום |
| הגדרות השרת | בעלים | פותח את §3 |
| יצירת ערוץ | בעלים | פותח את §4.1 |
| — מפריד — | | |
| עזיבת השרת | כולם (צבע `--danger`) | §2 |

הכפתור להעתקת הלינק שליד השם נשאר כקיצור.

- **תפריט:** `role="menu"` עם `aria-haspopup="menu"` ו-`aria-expanded` על הכפתור. חצים לניווט, Enter לבחירה, Esc סוגר ומחזיר פוקוס. נפתח מתחת לכפתור ברוחב פס הערוצים.
- **טלפון:** bottom sheet במסך מלא ברוחב, עם פריטים בגובה 48px.

## 2. B11a – עזיבת שרת

**חבר שאינו בעלים:** דיאלוג אישור.
- כותרת: `server.leave.title`
- טקסט: `server.leave.body`
- כפתורים: "עזיבה" (`server.leave.confirm`, מסוכן, ראשון בסדר ה-Tab) ו"ביטול".
- אחרי אישור: `leaveServer(serverId)` ← השרת יורד מהפס, עוברים ל"בית", ו-toast `server.leave.done`.

**בעלים (ש-19):** אותו פריט בתפריט, אבל הדיאלוג מסביר:
- כותרת: `server.leave.ownerTitle`
- טקסט: `server.leave.ownerBody`
- כפתורים: "להגדרות השרת" (ראשי, פותח את §3 ומגלגל ל"סגירת השרת") ו"סגירה".

## 3. B11b – הגדרות השרת (בעלים בלבד)

דיאלוג גדול (כמו הגדרות האפליקציה), עם שני עמודים בניווט: **"כללי"** ו**"ערוצים"**. בטלפון: מסך מלא, עם טאבים בראש.

### 3.1 "כללי"

1. **אייקון:** 80px עגול, "בחירת אייקון" ו"הסרה". אותו עיבוד כמו אווטאר ב-B11e §4 (‏256×256, ‏≤512KB, ש-22). בלי אייקון – ראשי התיבות על `color`.
2. **שם השרת** (חובה): `maxLength={48}`, מונה מ-40.
3. **תיאור** (לא חובה): `textarea`, 4 שורות, `maxLength={300}`, מונה מ-260. **לאישור מנהל המוצר:** מגבלת 300 התווים היא הצעה שלי.
4. **הגבלת גיל:** אותן 3 אפשרויות ואותו ניסוח של יצירת שרת (B7 / UX-15).
   - **העלאת** הגבלה (למשל ללא ← 18+) מציגה מתחת: `server.settings.ageRaiseWarning` (B7: מי שמתחת לגיל מאבדים גישה ומקבלים הודעה).
   - הורדת הגבלה – בלי אזהרה.
5. **לינק הזמנה** – §5.
6. שורת פעולות "שמירה" / "ביטול שינויים" (אותו דפוס כמו B11e §3: הרשומה נשלחת לכל החברים).
7. **אזור מסוכן** (בתחתית, מופרד בקו, גבול `--danger`): "סגירת השרת" – §3.2.

### 3.2 סגירת שרת (אישור כפול)

1. לחיצה על "סגירת השרת…" פותחת דיאלוג:
   - כותרת `server.close.title`
   - טקסט `server.close.body`
   - שדה: `server.close.typeName` – צריך להקליד את שם השרת בדיוק (השוואה אחרי `trim` ו-NFKC).
   - "סגירה סופית" (`server.close.confirm`, מסוכן) עם `aria-disabled` עד שהשם תואם, ו"ביטול".
2. אחרי אישור: `closeServer(serverId)` ← השרת יורד מהפס, עוברים ל"בית", ו-toast `server.close.done`.
3. **אצל החברים:** `server.removed` עם `reason: 'closed'` ← השרת יורד מהפס. אם הוא היה פתוח – עוברים ל"בית". toast: `server.closedByOwner`.

### 3.3 התיאור בהצטרפות (ש-23)

בדיאלוג ההצטרפות, אחרי "מחפשים חברים מהשרת…", כשנמצא חבר מחובר: **שלב תצוגה מקדימה** לפני ההצטרפות בפועל.
- אייקון (56px), שם, תג גיל, תיאור (עד 6 שורות, עם "עוד"), ומספר החברים (`join.preview.members`).
- כפתורים: "הצטרפות" (`join.preview.confirm`) ו"ביטול".
- אם הגיל לא מתאים – מוצג `join.ageBlocked` הקיים במקום הכפתור.
- **הצטרפות ברקע (B18):** כשהיא מגיעה לשלב הזה, העיגול בפס מקבל את אייקון השרת עם הטבעת המקווקוות ונקודה כחולה. toast: `join.preview.found` עם "לפרטים", שפותח את התצוגה המקדימה. ההצטרפות לא מסתיימת בלי אישור.

## 4. B11c – ערוצים (בעלים בלבד)

### 4.1 יצירה

- ליד הכותרת "ערוצים" בפס הערוצים: כפתור "+" (`channel.create`), רק לבעלים. גם מתפריט השרת.
- דיאלוג:
  - **שם הערוץ** (חובה): `maxLength={32}`. רווחים הופכים ל-`-` תוך כדי הקלדה (הרגל מדיסקורד), ורצף מקפים מתכווץ לאחד. מתחת לשדה: תצוגה "#שם-הערוץ" ב-`<bdi>`. **לאישור מנהל המוצר:** מגבלת 32 היא הצעה.
  - **"ערוץ למבוגרים (18+)"** – checkbox, עם `channel.adultHint`. אם לבעלים עצמם אין טענת 18+ – ה-checkbox מושבת עם `channel.adultNeeds18`.
  - "יצירה" / "ביטול".
- אחרי יצירה: הערוץ נפתח, והפוקוס בתיבת הכתיבה.
- שם תפוס בשרת (אחרי נרמול): `channel.error.nameTaken`.

### 4.2 עריכה

- כל ערוץ ברשימה, אצל הבעלים: אייקון גלגל שיניים בצד ה-end של השורה, שמופיע ב-hover או בפוקוס על השורה (בטלפון: תמיד מוצג), עם `aria-label` = `channel.edit`.
- גם בדף "ערוצים" בהגדרות השרת: רשימה של כל הערוצים, ולכל אחד "עריכה".
- דיאלוג העריכה: אותם שדות כמו ביצירה + "שמירה" + אזור מסוכן "מחיקת הערוץ".
- **#general (ש-20):** אפשר לשנות את השם ואת סימון ה-18+. **אין** כפתור מחיקה. במקומו שורה: `channel.cannotDeleteDefault`.

### 4.3 מחיקה (ש-20)

- דיאלוג אישור:
  - כותרת `channel.delete.title`
  - טקסט `channel.delete.body`
  - "מחיקת הערוץ" (מסוכן) / "ביטול".
- אחרי אישור: הערוץ נעלם מהרשימה. אם הוא היה פתוח – עוברים ל-#general.
- **אצל חבר שנמצא בערוץ באותו רגע:** עובר ל-#general, ו-toast `channel.deletedNotice`. אם הוא באמצע כתיבה – הטקסט נשאר בתיבה של #general, כדי שלא יאבד.

### 4.4 ערוץ 18+ (ש-12)

לחבר בלי טענת 18+ הערוץ לא נשלח מהליבה בכלל. אין בממשק "ערוץ נעול".

## 5. B11d – לינק הזמנה

בהגדרות השרת ← "כללי", בקטע "לינק הזמנה":
- הלינק הנוכחי ב-`Code` (בשורה אחת עם `…`), "העתקה", ו"ביטול הלינק…" (קישור מסוכן).
- "ביטול הלינק…" פותח דיאלוג:
  - כותרת `invite.revoke.title`
  - טקסט `invite.revoke.body`
  - "ביטול הלינק ויצירת חדש" (`invite.revoke.confirm`, מסוכן) / "ביטול".
- אחרי אישור: `revokeInvite(serverId)` ← הלינק החדש מופיע מייד, עם הדגשה קצרה (רקע `--accent-soft` ל-1.5 שניות), ו-toast `invite.revoke.done`.
- **מי שמנסים להצטרף עם הלינק הישן:** `join.status` עם `phase: 'revoked'` ← בדיאלוג ההצטרפות, או ב-toast אם זו הצטרפות ברקע: `join.revoked` (הניסוח של ש-21).
- **חברים קיימים:** לא מושפעים. כפתור ההעתקה שלהם מעתיק את הלינק החדש (`server.updated`).

## 6. מצבים

| מצב | מה רואים |
| --- | --- |
| טוען (שמירה, יצירה, מחיקה, ביטול לינק) | הכפתור הראשי מושבת עם spinner, והשדות `readonly` |
| שגיאה: `not-allowed` | `server.error.notAllowed` מעל הכפתורים. קורה אם הבעלות השתנתה (שלב 2) או שהממשק לא מסונכרן |
| שגיאה: שם ריק או תפוס | מתחת לשדה, ופוקוס עליו |
| שגיאה כללית | `error.generic` עם "לנסות שוב" |
| אופליין (אף אחד בשרת לא מחובר) | כל הפעולות מצליחות מקומית ונשלחות כשמישהו מתחבר (כמו הודעה). אחרי שמירה ב"הגדרות השרת" מופיעה השורה `server.settings.pendingSync` עד שהרשומה יוצאת |
| עזיבה או סגירה כשאף אחד לא מחובר | השרת יורד מהמחשב הזה מייד. רשומת הסגירה או העזיבה יוצאת כשמישהו מתחבר |
| שם שרת ארוך (48 תווים) ב-200% / 24px | בכפתור שבראש פס הערוצים: שורה אחת עם `…` ו-`title` מלא. בכותרת התפריט: עד 2 שורות. בדיאלוגים: השם ב-`<bdi>`, ושבירת שורה רגילה |
| שם ערוץ ארוך (32 תווים) | ברשימה: `…`. בכותרת הצ'אט: `…`, עם `title` |
| טלפון 375px | התפריט כ-bottom sheet. הגדרות השרת במסך מלא עם טאבים. כל הדיאלוגים במסך מלא, עם הכפתורים בתחתית |

## 7. מקלדת וקורא מסך

- תפריט השרת – §1.
- גלגל השיניים של ערוץ: מופיע גם ב-`:focus-within` של השורה, כך שאפשר להגיע אליו ב-Tab. הוא לא חלק מה-roving של רשימת הערוצים – Tab אחד מהערוץ הנוכחי.
- כל דיאלוג מסוכן: הפוקוס הראשוני על "ביטול", לא על הפעולה המסוכנת. Esc = ביטול.
- "סגירה סופית" עם `aria-disabled` עד שהשם תואם. ה-`aria-describedby` שלו מצביע על `server.close.typeName`.
- הודעות על שינויים מבחוץ (ערוץ נמחק, שרת נסגר, הוסרת): `role="status"` (toast), והפוקוס עובר לכותרת הצ'אט של #general או של "בית".

## 8. CoreApi

| פקודה / אירוע / טיפוס | שינוי | הערה |
| --- | --- | --- |
| `ServerSummary` | **שינוי חוזה:** מוסיפים `iconDataUrl?: string`, `description?: string` | B4: רשומת עריכת שרת |
| `Channel` | **שינוי חוזה:** מוסיפים `isDefault: boolean` | ‏#general. לא לזהות לפי השם, כי אפשר לשנות אותו |
| `leaveServer(serverId): Promise<void>` | **חדש** | נדחה עם `not-allowed` לבעלים (ש-19). מוחק את העותק המקומי |
| `updateServer(serverId, { name?, iconDataUrl?: string \| null, description?, ageRestriction? }): Promise<ServerSummary>` | **חדש** | `null` = הסרת אייקון. נדחה עם `not-allowed`, `invalid-name` או `too-large` |
| `closeServer(serverId): Promise<void>` | **חדש** | רשומת סגירה חתומה |
| `createChannel(serverId, { name, adult }): Promise<Channel>` | **חדש** | נדחה עם `name-taken` (קוד חדש) או `invalid-name` |
| `updateChannel(channelId, { name?, adult? }): Promise<Channel>` | **חדש** | |
| `deleteChannel(channelId): Promise<void>` | **חדש** | נדחה עם `not-allowed` על `isDefault` |
| `revokeInvite(serverId): Promise<ServerSummary>` | **חדש** | מחזיר את השרת עם `inviteLink` חדש (`mvs://join/…`) |
| `server.removed` | **שינוי חוזה:** `{ serverId, name, reason: 'left' \| 'closed' \| 'kicked' }` | `name` ל-toast, כי השרת כבר לא ברשימה |
| `channel.removed` | **אירוע חדש:** `{ serverId, channelId, name }` | |
| `channel.added` | **אירוע חדש:** `Channel` | היום יש רק `channel.updated` |
| `JoinStatus` | **שינוי חוזה:** מוסיפים `{ phase: 'preview'; link; server: { name, iconDataUrl?, ageRestriction, description?, memberCount } }` ו-`{ phase: 'revoked'; link }` | ש-23, ש-21 |
| `confirmJoin(link): Promise<JoinStatus>` | **חדש** | אחרי `preview`. מחזיר `joined` או `age-blocked` |
| `CoreError['code']` | **שינוי חוזה:** מוסיפים `'name-taken'` (ו-`'invalid-name'`, `'too-large'` מ-B11e) | |

## 9. מחרוזות

| מפתח | עברית | English |
| --- | --- | --- |
| `server.menu` | `אפשרויות שרת` (ללא שינוי) | `Server options` |
| `server.settings.open` | `הגדרות השרת` | `Server settings` |
| `server.settings.general` | `כללי` | `General` |
| `server.settings.channels` | `ערוצים` | `Channels` |
| `server.settings.icon` | `אייקון השרת` | `Server icon` |
| `server.settings.iconPick` | `בחירת אייקון` | `Choose an icon` |
| `server.settings.description` | `תיאור (לא חובה)` | `Description (optional)` |
| `server.settings.descriptionHint` | `כל חברי השרת רואים את התיאור, וגם מי שמצטרפים בלינק, לפני שהם מאשרים.` | `All members see the description, and so do people joining with a link, before they confirm.` |
| `server.settings.ageRaiseWarning` | `חברים שמתחת לגיל ⁨{age}⁩ יאבדו גישה לשרת ויקבלו הודעה על כך.` | `Members under ⁨{age}⁩ will lose access to the server and get a notice.` |
| `server.settings.pendingSync` | `השינויים יגיעו לחברים כשמישהו מהשרת יתחבר.` | `Members get the changes when someone from the server connects.` |
| `server.settings.danger` | `אזור מסוכן` | `Danger zone` |
| `server.leave` | `עזיבת השרת` | `Leave server` |
| `server.leave.title` | `לעזוב את ⁨{server}⁩?` | `Leave ⁨{server}⁩?` |
| `server.leave.body` | `השרת וכל מה שנשמר ממנו יימחקו מהמחשב הזה. השרת ימשיך לפעול אצל שאר החברים, ואפשר לחזור עם לינק הזמנה.` | `The server and everything saved from it are deleted from this computer. It keeps running for the other members, and you can come back with an invite link.` |
| `server.leave.confirm` | `עזיבה` | `Leave` |
| `server.leave.done` | `השרת ⁨{server}⁩ הוסר מהמחשב הזה.` | `⁨{server}⁩ was removed from this computer.` |
| `server.leave.ownerTitle` | `הבעלים לא יכולים לעזוב את השרת` | `Owners can't leave their server` |
| `server.leave.ownerBody` | `עוד אין אפשרות להעביר בעלות. אפשר להישאר בשרת, או לסגור אותו אצל כל החברים בהגדרות השרת.` | `Ownership can't be transferred yet. You can stay, or close the server for everyone in server settings.` |
| `server.leave.ownerAction` | `להגדרות השרת` | `Go to server settings` |
| `server.close` | `סגירת השרת…` | `Close server…` |
| `server.close.title` | `לסגור את ⁨{server}⁩ אצל כולם?` | `Close ⁨{server}⁩ for everyone?` |
| `server.close.body` | `השרת יוסר מהרשימה של כל החברים, וההודעות יימחקו במחשבים שמריצים גרסה תקינה של MVS. אי אפשר לבטל את זה.` | `The server is removed for all members, and messages are deleted on computers running a normal version of MVS. This can't be undone.` |
| `server.close.typeName` | `כדי לאשר, צריך להקליד את שם השרת: ⁨{server}⁩` | `To confirm, type the server name: ⁨{server}⁩` |
| `server.close.confirm` | `סגירה סופית` | `Close for good` |
| `server.close.done` | `השרת ⁨{server}⁩ נסגר.` | `⁨{server}⁩ was closed.` |
| `server.closedByOwner` | `הבעלים סגרו את השרת ⁨{server}⁩.` | `The owner closed ⁨{server}⁩.` |
| `server.error.notAllowed` | `רק הבעלים יכולים לעשות את זה.` | `Only the owner can do this.` |
| `channel.create` | `יצירת ערוץ` | `Create channel` |
| `channel.edit` | `עריכת הערוץ` | `Edit channel` |
| `channel.name` | `שם הערוץ` | `Channel name` |
| `channel.adultHint` | `הערוץ מוסתר מחברים בני 16–17.` | `Hidden from members aged 16–17.` |
| `channel.adultNeeds18` | `רק מי שבני 18 ומעלה יכולים לסמן ערוץ למבוגרים.` | `Only people 18 or older can mark a channel as adults-only.` |
| `channel.error.nameTaken` | `כבר יש ערוץ בשם הזה.` | `There's already a channel with this name.` |
| `channel.cannotDeleteDefault` | `אי אפשר למחוק את הערוץ הראשון של השרת, אבל אפשר לשנות לו שם.` | `The server's first channel can't be deleted, but you can rename it.` |
| `channel.delete` | `מחיקת הערוץ` | `Delete channel` |
| `channel.delete.title` | `למחוק את #⁨{channel}⁩?` | `Delete #⁨{channel}⁩?` |
| `channel.delete.body` | `הערוץ וכל ההודעות בו יימחקו אצל כולם, במחשבים שמריצים גרסה תקינה של MVS. אי אפשר לבטל את זה.` | `The channel and all its messages are deleted for everyone, on computers running a normal version of MVS. This can't be undone.` |
| `channel.deletedNotice` | `הערוץ #⁨{channel}⁩ נמחק.` | `#⁨{channel}⁩ was deleted.` |
| `invite.section` | `לינק הזמנה` | `Invite link` |
| `invite.revoke` | `ביטול הלינק…` | `Revoke link…` |
| `invite.revoke.title` | `לבטל את לינק ההזמנה?` | `Revoke the invite link?` |
| `invite.revoke.body` | `מי שינסו להצטרף עם הלינק הנוכחי יקבלו הודעה שהוא בוטל. חברים קיימים לא יושפעו. לינק חדש ייווצר מייד.` | `Anyone who tries to join with the current link is told it was revoked. Existing members aren't affected. A new link is created right away.` |
| `invite.revoke.confirm` | `ביטול הלינק ויצירת חדש` | `Revoke and create a new link` |
| `invite.revoke.done` | `נוצר לינק הזמנה חדש.` | `A new invite link was created.` |
| `join.revoked` | `הלינק הזה בוטל. אפשר לבקש לינק חדש.` | `This link was revoked. Ask for a new one.` |
| `join.preview.members` | `{count, plural, one {חבר אחד} two {שני חברים} other {# חברים}}` | `{count, plural, one {# member} other {# members}}` |
| `join.preview.confirm` | `הצטרפות` | `Join` |
| `join.preview.found` | `השרת ⁨{server}⁩ נמצא.` | `Found ⁨{server}⁩.` |
| `join.preview.details` | `לפרטים` | `Details` |

הערה על `join.preview.members`: "חבר אחד" הוא שם עצם ממוגדר ביחיד. כאן הוא מונה ולא פנייה לאדם, ולכן לפי הכלל הוא בסדר. **לאישור מנהל המוצר.** חלופה ניטרלית: "{count} בשרת".

## 10. בדיקות קבלה

**אוטומטיות:**
1. **B11a:** חבר ← "עזיבת השרת" ← "עזיבה" ← השרת לא בפס, ו-`leaveServer` נקרא. בעלים ← אותו פריט ← דיאלוג ההסבר, ואין קריאה ל-`leaveServer`.
2. **B11b:** שינוי שם ותיאור ← `server.updated` אצל חבר אחר בדמה ← השם החדש בפס, והתיאור בכותרת התפריט.
3. **סגירה:** "סגירה סופית" עם `aria-disabled` עד הקלדת השם המדויק. אחרי סגירה – `server.removed` עם `reason: 'closed'` אצל חבר בדמה, ו-toast.
4. **B11c:** יצירת "חדשות משחקים" ← השם נשמר כ-"חדשות-משחקים". שם קיים ← `channel.error.nameTaken`. ב-#general אין כפתור מחיקה.
5. **מחיקת ערוץ פתוח אצל חבר** ← החבר עובר ל-#general, ו-toast. טקסט שהיה בתיבה נשאר.
6. **B11d:** "ביטול הלינק ויצירת חדש" ← `inviteLink` חדש מופיע. הצטרפות עם הלינק הישן ← `join.revoked`.
7. **ש-23:** הצטרפות ← שלב תצוגה מקדימה עם התיאור לפני `confirmJoin`.
8. **בעלים בני 17 (דמה)** ← ה-checkbox של 18+ מושבת.

**צילומים:**

| # | צילום | מה רואים |
| --- | --- | --- |
| 110 | `110-server-menu-owner-he` | תפריט פתוח: כותרת עם תיאור, ופריטי בעלים |
| 111 | `111-server-menu-member-en` | התפריט של חבר, באנגלית |
| 112 | `112-leave-confirm-he` | אישור עזיבה לחבר |
| 113 | `113-leave-owner-blocked-he` | הסבר לבעלים, עם "להגדרות השרת" |
| 114 | `114-server-settings-general-he` | אייקון, שם, תיאור, גיל, לינק ואזור מסוכן |
| 115 | `115-server-close-confirm-he` | אישור כפול עם שדה השם, והכפתור מושבת |
| 116 | `116-channel-create-he` | דיאלוג יצירה עם תצוגת "#חדשות-משחקים" ו-checkbox 18+ |
| 117 | `117-channel-edit-general-he` | עריכת #general: אין מחיקה, ומופיעה השורה המסבירה |
| 118 | `118-channel-delete-confirm-he` | אישור מחיקה |
| 119 | `119-invite-revoke-he` | דיאלוג ביטול הלינק |
| 120 | `120-join-preview-he` | תצוגה מקדימה בהצטרפות: אייקון, שם, גיל, תיאור |
| 121 | `121-join-revoked-he` | הצטרפות עם לינק שבוטל |
| 122 | `122-server-settings-phone-he` | 375px: הגדרות שרת במסך מלא עם טאבים |
| 123 | `123-server-menu-200` | 200% + 24px: שם שרת של 48 תווים בכותרת התפריט, בלי גלישה |
