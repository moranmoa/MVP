# Changelog

What changed in each MVS release, newest first. The release workflow puts
the section of the tagged version into the release notes, and a tag build
fails if its version has no section here. Write a section before tagging:
`## [X.Y.Z] - YYYY-MM-DD`, then English (New / Fixed / Known).
Management text is English only (owner, 2026-11-11); the bilingual
sections up to 0.4.0 stay as history.
New sections must not contain a `---` line: the release scripts still read
it as the old Hebrew/English split.
Never put the relay domain or any IP address here.

## [0.4.2] - 2026-11-11

### New
- **Your picture and colour everywhere:** your profile picture now shows next to your messages, in the member list and in voice, for everyone in your servers. Without a picture, your initial shows on your colour. Pick your colour in Settings → Account.
- **New window and logo:** MVS has its own title bar and its new logo on the window, taskbar and installer. Minimising goes to the taskbar.
- **Cleaner server header:** the age tag and the invite button moved out of the header. The invite link is in server settings and the server's right-click menu.
- **Language list:** a 🌐 icon next to the language picker.

### Fixed
- **Who's online:** members connected to the server no longer show as offline, and the "Nobody from the server is online" banner no longer appears by mistake.
- **Unread dots that stayed:** the unread dot and count on channels and servers now clear when you read them.

### Known
- **Voice calls are officially supported only when everyone is on the same network** (for example the same Wi-Fi). Calls across networks are on the way.
- On first run, Windows may ask whether to allow MVS on the network. Please allow it (also on private networks); otherwise voice calls won't connect.

## [0.4.1] - 2026-11-11

### New
- **Language follows your system:** the app opens in your computer's language when it's available (Hebrew or English), otherwise in English.
- **Language picker:** choose the language from a list in Settings → Language and on the first screen. More languages will be added to the same list.
- **Release notes in English:** from this version on, the "What's new" notes are written in English.

### Fixed
- **Former members in mentions:** the "@former account" label now has a clear border and background.

### Known
- **Voice calls are officially supported only when everyone is on the same network** (for example the same Wi-Fi). Calls across networks are on the way.
- On first run, Windows may ask whether to allow MVS on the network. Please allow it (also on private networks); otherwise voice calls won't connect.

## [0.4.0] - 2026-11-11

### מה חדש
- **סדר השרתים שלך:** אפשר לגרור שרתים ברשימה (או Alt+Shift+חצים, או "שינוי סדר השרתים" בטלפון). הסדר נשמר רק במחשב הזה.
- **סדר הערוצים בשרת:** הבעלים יכולים לשנות את סדר הערוצים, וכל החברים רואים את הסדר החדש. גרסאות ישנות פשוט ממשיכות להציג את סדר היצירה.
- **"הודעות חדשות":** קו מפריד מסמן מאיפה מתחילות ההודעות שלא קראת, ופס "N חדשות · מעבר" קופץ אליהן.
- **אזכורים עם @:** כשמקלידים @ מופיעה רשימת חברים לבחירה. אזכור מוצג כשם האדם, ומי שעזב מוצג כ"@חשבון שעזב". על הערוץ ועל השרת מופיע מונה אזכורים שלא נקראו.
- **השתקה וסימון כנקרא:** אפשר להשתיק שרת או ערוץ לשעה, 8 שעות, 24 שעות או עד שמבטלים, ולסמן שרת שלם כנקרא.
- **התראות בדסקטופ:** בחלון "התראות" של שרת או ערוץ בוחרים רמה (הכול / רק אזכורים / כלום) והשתקה. כברירת מחדל ההתראה לא מציגה את תוכן ההודעה, רק "הודעה חדשה ב-#ערוץ". אזכור שלך מגיע גם כשהערוץ מושתק.
- **השרתים חוזרים אחרי שחזור:** רשימת השרתים נשמרת מוצפנת וחתומה ברשת, ושחזור ממילות השחזור במכשיר חדש מצטרף אליהם מחדש כשאחד המחשבים שלך מחובר.

### מה תוקן
- **הצטרפות שנתקעת:** פחות בקשות מיותרות בין מחשבים, גם אחרי ניתוק וחיבור מחדש דרך הממסר.

### ידוע
- **שיחות קול נתמכות רשמית רק כשכל המשתתפים באותה רשת** (למשל אותו Wi-Fi). שיחות בין רשתות שונות בדרך.
- בפעם הראשונה Windows עשוי לשאול אם לאפשר ל-MVS גישה לרשת. מומלץ לאשר (גם ברשתות פרטיות); בלי זה שיחות קול לא יתחברו.

---

### New
- **Your server order:** drag servers in the list (or Alt+Shift+arrows, or "Reorder servers" on a phone). The order is saved on this computer only.
- **Channel order in a server:** owners can reorder channels, and every member sees the new order. Older versions simply keep showing the creation order.
- **"New messages":** a divider marks where your unread messages start, and an "N new · Jump" bar takes you there.
- **@mentions:** typing @ lists members to pick from. A mention shows the person's name, and someone who left shows as "@former account". Channels and servers show a count of unread mentions.
- **Mute and mark as read:** mute a server or channel for 1 hour, 8 hours, 24 hours or until you turn it back on, and mark a whole server as read.
- **Desktop notifications:** in a server's or channel's "Notifications" window, choose a level (all / mentions only / none) and mute. By default a notification doesn't show the message text, only "New message in #channel". A mention of you still notifies when the channel is muted.
- **Servers come back after a restore:** your server list is saved encrypted and signed on the network, and restoring from your recovery words on a new device rejoins them when one of your other computers is online.

### Fixed
- **Joins that got stuck:** fewer needless requests between computers, also after reconnecting through the relay.

### Known
- **Voice calls are officially supported only when everyone is on the same network** (for example the same Wi-Fi). Calls across networks are on the way.
- On first run, Windows may ask whether to allow MVS on the network. Please allow it (also on private networks); otherwise voice calls won't connect.

## [0.3.3] - 2026-11-10

### מה חדש
- **יציאה מהחשבון במחשב הזה:** בהגדרות → חשבון. מקלידים את 12 מילות השחזור, והחשבון, השרתים וההודעות נמחקים מהמחשב הזה בלבד. החשבון עצמו לא נמחק, ואפשר לשחזר אותו עם המילים. יציאה בלי המילים דורשת אישור נוסף, כי בלעדיהן אי אפשר לשחזר את החשבון.
- **שיחה ממחשב אחד:** אם כבר נמצאים בשיחה במחשב אחר, מופיעה הודעה שמסבירה את זה, במקום חיבור כפול.
- **תאריך לידה בהגדרות:** בהגדרות → חשבון מוצג תאריך הלידה. תיקון שמעלה את הגיל נכנס לתוקף אחרי 30 יום, והתאריך שבו זה יקרה מוצג.

### מה תוקן
- **הצטרפות לשרת שנתקעת:** מחשב שנמצא בהרבה שרתים כבר לא מציף את המחשבים האחרים בבקשות, ולכן הצטרפות לשרת שנוצר בו לא נתקעת יותר. בבדיקות: הצטרפות בפחות משנייה גם עם 100 שרתים.
- **שחזור וגיל:** שחזור חשבון עם תאריך לידה שמעלה את הגיל לא נותן גישה מיידית לשרתים ל-18+. יום הולדת 18 אמיתי נכנס לתוקף מיד.
- **קובץ היומן:** שמות פנימיים כמו mvs::join נשמרים קריאים; כתובות IP עדיין מוסתרות.

### ידוע
- **שיחות קול נתמכות רשמית רק כשכל המשתתפים באותה רשת** (למשל אותו Wi-Fi).
- בפעם הראשונה Windows עשוי לשאול אם לאפשר ל-MVS גישה לרשת. מומלץ לאשר (גם ברשתות פרטיות); בלי זה שיחות קול לא יתחברו.

---

### New
- **Sign out on this computer:** in Settings → Account. You type your 12 recovery words, and the account, servers and messages are removed from this computer only. The account itself isn't deleted, and you can restore it with the words. Signing out without the words needs an extra confirmation, because without them the account can't be restored.
- **One computer per call:** if you're already in a call on another computer, a message explains it instead of joining twice.
- **Date of birth in Settings:** Settings → Account shows your date of birth. A correction that raises your age takes effect after 30 days, and the date it does is shown.

### Fixed
- **Joining a server that got stuck:** a computer that's in many servers no longer floods other computers with requests, so joining a server created on it doesn't get stuck anymore. In tests: joins in under a second, even with 100 servers.
- **Restore and age:** restoring an account with a birth date that raises the age doesn't give immediate access to 18+ servers. A real 18th birthday takes effect at once.
- **Log file:** internal names like mvs::join stay readable; IP addresses are still hidden.

### Known
- **Voice calls are officially supported only when everyone is on the same network** (for example the same Wi-Fi).
- On first run, Windows may ask whether to allow MVS on the network. Please allow it (also on private networks); otherwise voice calls won't connect.

## [0.3.2] - 2026-11-10

### מה חדש
- **עריכת פרופיל:** בהגדרות → חשבון אפשר לשנות שם, תמונה וביו, עם כפתור "שמירה". שם חדש מתעדכן אצל כל מי שנמצא איתך בשרתים. את התמונה והביו רואים בינתיים רק במחשב הזה.
- **שחזור חשבון:** בשחזור ממילות השחזור האפליקציה שואלת שוב את ההסכמה, המדינה ותאריך הלידה, ואז את השם. אחרי השחזור, החזרה לשרת שומרת את השם שלך.

### מה תוקן
- **הודעות שממתינות למפתח הערוץ:** הודעה שנשלחה רגע אחרי הצטרפות כבר לא נכשלת. היא מסומנת "נשלחת" ויוצאת כשמפתח הערוץ מגיע. אם היא לא יצאה, אפשר "לנסות שוב" או למחוק אותה.
- **אבחון הצטרפות:** כל שלב בהצטרפות לשרת נרשם בקובץ היומן של האפליקציה (בלי תוכן הודעות), כדי שנוכל למצוא למה הצטרפות נתקעת.

### ידוע
- **הצטרפות לשרת שנוצר במחשב מסוים עלולה להיתקע** בחלק מהרשתות הביתיות. אנחנו בודקים את זה. בינתיים, אפשר ליצור את השרת במחשב השני.
- **שיחות קול נתמכות רשמית רק כשכל המשתתפים באותה רשת** (למשל אותו Wi-Fi).
- בפעם הראשונה Windows עשוי לשאול אם לאפשר ל-MVS גישה לרשת. מומלץ לאשר (גם ברשתות פרטיות); בלי זה שיחות קול לא יתחברו.
- **אין עדיין יציאה מהחשבון במחשב.** הסרת האפליקציה לא מוחקת את החשבון, כי המפתחות שמורים במנהל האישורים של Windows.

---

### New
- **Edit your profile:** in Settings → Account you can change your name, picture and bio, with a "Save" button. A new name updates for everyone in your servers. For now, the picture and bio show only on this computer.
- **Restoring an account:** restoring from your recovery words asks for the agreement, country and birth date again, then your name. After a restore, rejoining a server keeps your name.

### Fixed
- **Messages waiting for the channel key:** a message sent right after joining no longer fails. It shows as "sending" and goes out when the channel key arrives. If it doesn't go out, you can "Try again" or delete it.
- **Join diagnostics:** every step of joining a server is written to the app's log file (no message content), so we can find why a join gets stuck.

### Known
- **Joining a server created on a particular computer may get stuck** on some home networks. We're looking into it. Meanwhile, create the server on the other computer.
- **Voice calls are officially supported only when everyone is on the same network** (for example the same Wi-Fi).
- On first run, Windows may ask whether to allow MVS on the network. Please allow it (also on private networks); otherwise voice calls won't connect.
- **There's no sign-out on a computer yet.** Uninstalling the app doesn't remove the account, because its keys are kept in Windows Credential Manager.

## [0.3.1] - 2026-11-10

### מה תוקן
- **התקנה ב-Windows:** האפליקציה נפתחת גם במחשב שאין בו את רכיב ה-Visual C++ של Microsoft. ב-0.3.0 הופיעה במחשבים כאלה השגיאה "VCRUNTIME140.dll was not found".
- **ההודעה הראשונה אחרי הצטרפות:** לא נדחית יותר בחצי השנייה הראשונה אחרי ההצטרפות לשרת.

### ידוע
- **שיחות קול עובדות כרגע רק כשכל המשתתפים באותה רשת** (למשל אותו Wi-Fi). אנחנו עובדים על שיחות בין רשתות שונות.
- בפעם הראשונה Windows עשוי לשאול אם לאפשר ל-MVS גישה לרשת. מומלץ לאשר (גם ברשתות פרטיות); בלי זה שיחות קול לא יתחברו.

---

### Fixed
- **Installing on Windows:** the app now opens on computers without Microsoft's Visual C++ runtime. In 0.3.0 such computers showed "VCRUNTIME140.dll was not found".
- **Your first message after joining** is no longer refused in the first half-second after joining a server.

### Known
- **Voice calls currently work only when everyone is on the same network** (for example the same Wi-Fi). Calls across networks are in progress.
- On first run, Windows may ask whether to allow MVS on the network. Please allow it (also on private networks); otherwise voice calls won't connect.

## [0.3.0] - 2026-11-10

### מה חדש
- **שיחות קול:** בכל שרת יש חדר קולי "הצטרפות לשיחה" עם סמל רמקול. כל מי שבשרת יכולים להיכנס, עד 5 אנשים, ורואים מי בפנים עוד לפני שנכנסים. יש השתקה, יציאה, וסימון מי מדבר.
- **פרטיות בשיחות:** לפני השיחה הראשונה מופיעה הודעה שבשיחה המחשבים מתחברים ישירות, ולכן כתובת ה-IP גלויה למשתתפים. במצב "ממסר בלבד" אין שיחות.
- **מיקרופון רק בזמן שיחה:** האפליקציה מקבלת גישה למיקרופון רק אחרי הצטרפות לשיחה. בגרסה הזו האפליקציה לא ניגשת למצלמה, למסך או לשמע של המחשב.
- **שינוי שם, יציאה וסגירה של שרת:** הבעלים יכולים לשנות לו שם ולסגור אותו (השרת נשאר לקריאה בלבד 30 יום, ואחר כך הוא כבר לא מופיע באפליקציה). אפשר לצאת משרת.
- **תפריטים בטלפון:** בטלפון התפריטים נפתחים כגיליון מלמטה.
- **רישיונות:** כל התקנה כוללת קובץ רישיונות של רכיבי הקוד הפתוח.

### מה תוקן
- **ההודעה הראשונה אחרי הצטרפות** נשלחת מהר יותר, גם כשכמה אנשים מצטרפים בבת אחת.
- **עדכון ספריית הרשת** (libp2p 0.56): הוסרו רכיבי הצפנה ישנים עם 7 התראות אבטחה.
- **Windows:** אם רכיב הליבה חסר, מוצג מסך "חסר" במקום קריסות חוזרות.

### ידוע
- **שיחות קול עובדות כרגע רק כשכל המשתתפים באותה רשת** (למשל אותו Wi-Fi). אנחנו עובדים על שיחות בין רשתות שונות.
- בפעם הראשונה Windows עשוי לשאול אם לאפשר ל-MVS גישה לרשת. מומלץ לאשר (גם ברשתות פרטיות); בלי זה החיבור עובר רק דרך הממסר, ושיחות קול לא יתחברו. אפשר לשנות את זה אחר כך בהגדרות חומת האש של Windows.

---

### New
- **Voice calls:** every server has a "Join chat" voice room with a speaker icon. Anyone in the server can join, up to 5 people, and you can see who is inside before joining. Mute, leave, and who's speaking.
- **Privacy in calls:** before your first call, a notice explains that in a call computers connect directly, so your IP address is visible to the other participants. No calls in relay-only mode.
- **Microphone only during a call:** the app gets the microphone only after you join a call. In this version the app doesn't access the camera, the screen or the computer's sound.
- **Rename, leave and close a server:** owners can rename a server and close it (it stays read-only for 30 days, then no longer shows in the app). You can leave a server.
- **Phone menus:** on a phone, menus open as a sheet from the bottom.
- **Licences:** every install includes the open-source licence notices.

### Fixed
- **Your first message after joining** goes through faster, also when several people join at once.
- **Network library upgrade** (libp2p 0.56): old crypto components with 7 security advisories are gone.
- **Windows:** if the core component is missing, the app shows a "missing" screen instead of crash-looping.

### Known
- **Voice calls currently work only when everyone is on the same network** (for example the same Wi-Fi). Calls across networks are in progress.
- On first run, Windows may ask whether to allow MVS on the network. Please allow it (also on private networks); otherwise connections go through the relay only, and voice calls won't connect. You can change this later in Windows Firewall settings.

## [0.2.5] - 2026-11-06

גרסה 0.2.4 לא פורסמה, ולכן 0.2.5 כוללת גם את כל מה שבה.

### מה חדש
- **ערוצים:** בעל השרת יכול ליצור ערוצים, לשנות להם שם ולמחוק אותם.
- **הגדרות שרת:** כפתור גלגל שיניים ליד שם השרת פותח מסך הגדרות עם ערוצים, חברים וקישורי הזמנה.
- **קישורי הזמנה:** יצירת קישור עם תוקף ומספר שימושים, רשימת כל הקישורים וביטול קישור.
- **תפריט קליק ימני:** על ערוצים, חברים, שרתים והודעות (תשובה, העתקה, עריכה, מחיקה).
- **סמל "ישיר" / "דרך relay":** ליד כל חבר מחובר מופיע סמל שמראה אם החיבור אליו ישיר או עובר דרך ה-relay.
- **פורט האזנה קבוע** וניסיון חיבור ישיר משני הצדדים.
- **תשתית לעדכון אוטומטי:** מגרסה זו האפליקציה יודעת לבדוק, להוריד ולהתקין עדכונים חתומים, כולל ערוץ בטא לגרסאות מקדימות.

### מה תוקן
- **הודעות מגיעות מיד** (בטיוטת 0.2.3 הייתה השהיה של כ-5 שניות).
- **הצטרפות לשרת:** אם לא נמצא אף חבר מחובר, האפליקציה ממשיכה לנסות ברקע עד 10 דקות במקום לוותר אחרי 5 שניות.

### ידוע
- את 0.2.5 מתקינים ידנית מעל הגרסה הקיימת. העדכון האוטומטי יעבוד מהגרסה הבאה ואילך.
- עדיין אין "ניקוב" חיבור דרך NAT בין רשתות שונות (hole punching).
- שינוי שם שרת, יציאה משרת ומחיקת שרת יגיעו בגרסה הבאה.
- שיחות קול עדיין לא זמינות.

---

Version 0.2.4 was never published, so 0.2.5 also includes everything in it.

### New
- **Channels:** the server owner can create channels, rename them and delete them.
- **Server settings:** a gear next to the server name opens a settings screen with channels, members and invite links.
- **Invite links:** create a link with an expiry and a use limit, see all links, and revoke one.
- **Right-click menus:** on channels, members, servers and messages (reply, copy, edit, delete).
- **"Direct" / "Via relay" icon:** next to each online member, an icon shows whether you're connected directly or through the relay.
- **A stable listening port**, and a direct connection tried from both sides.
- **Auto-update groundwork:** from this version the app can check for, download and install signed updates, including a beta channel for pre-releases.

### Fixed
- **Messages arrive instantly** (the 0.2.3 draft delayed each one by about 5 seconds).
- **Joining a server:** if no member is online yet, the app keeps trying in the background for up to 10 minutes instead of giving up after 5 seconds.

### Known
- Install 0.2.5 by hand over your current version. Auto-update works from the next version on.
- No NAT hole punching between different networks yet.
- Renaming, leaving and deleting a server come in the next version.
- Voice calls are not available yet.

## [0.2.4] - 2026-11-05

### מה חדש
- **סימון "ישיר" / "דרך relay":** ברשימת החברים, ליד כל חבר מחובר מופיע איך אתם מחוברים אליו.
- **פורט האזנה קבוע:** האפליקציה בוחרת פורט בהפעלה הראשונה ושומרת אותו גם אחרי הפעלה מחדש, כך שאפשר לאשר אותו פעם אחת ב-Firewall או בראוטר.
- **ניסיון חיבור ישיר משני הצדדים:** אם החיבור הישיר מהצד הראשון נכשל, הצד השני מנסה בעצמו.
- **לוג מפורט לפתרון תקלות:** זמין כשצריך לאבחן בעיה.

### מה תוקן
- **הודעות מגיעות שוב מיד.** בטיוטה של 0.2.3 (שלא פורסמה) הייתה השהיה של כ-5 שניות בכל הודעה.
- **הצטרפות לשרת מיידית שוב:** אפשר לכתוב מיד אחרי ההצטרפות.

### ידוע
- עדיין אין "ניקוב" חיבור דרך NAT בין רשתות שונות (hole punching); זה הבא בתור.
- יצירת ערוצים, שינוי שם ומחיקה יגיעו לאפליקציה בגרסה הבאה.
- מצב "relay בלבד" (שלא חושף את כתובת הרשת שלכם לחברים): יש לו רק תשתית ראשונית, ועוד אי אפשר להפעיל אותו.
- העדכון האוטומטי בתוך האפליקציה עוד לא פעיל: מתקינים את הגרסה החדשה ידנית מעל הקיימת.

---

### New
- **"Direct" / "Via relay" tag:** in the member list, each online member shows how you're connected to them.
- **A stable listening port:** the app picks a port on first start and keeps it across restarts, so you can allow it once in your firewall or router.
- **Direct connection tried from both sides:** if the first side's direct connection fails, the other side tries.
- **Detailed logging for troubleshooting:** available when a problem needs diagnosing.

### Fixed
- **Messages arrive instantly again.** The 0.2.3 draft, which was never published, delayed every message by about 5 seconds.
- **Joining a server is immediate again:** you can write right after joining.

### Known
- No NAT hole punching between different networks yet; that's next.
- Creating, renaming and deleting channels arrives in the app in the next version.
- A "relay only" mode (which doesn't reveal your network address to other members) is groundwork only and isn't available yet.
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
