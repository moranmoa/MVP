# ניסוי קול – הוראות לבודקים (6 אנשים)

המטרה: לשמוע איך נשמעת שיחה קבוצתית בשתי הדרכים שבהן האפליקציה יכולה לעבוד,
על מחשבים אמיתיים, מיקרופונים אמיתיים ורשתות ביתיות. בבדיקות במעבדה (Docker)
אין רמקולים ואין הד, אז את זה רק אתם יכולים לבדוק.

| גרסה | מה זה | איך מריצים |
| --- | --- | --- |
| **A – Electron** | הקול עובר במנוע של Chromium (כמו Discord) | אפליקציה עם חלון |
| **B – מנוע Rust** | הקול עובר במנוע שלנו (str0m), כמו שיהיה ב-Tauri | חלון טרמינל |

הבדיקה לוקחת כ-20 דקות: שלושה סבבים של 5 דקות.

## מה צריך

- **אוזניות** – חובה. גרסה B עדיין בלי ביטול הד (אלא אם בונים אותה עם `apm`, ראו למטה).
- כתובת **שרת האיתות** וגם **שם חדר** – מורן ישלח. הכתובת נראית כך: `ws://1.2.3.4:8080`.
- את התיקייה `spikes/voice` מהמאגר (`git clone https://github.com/moranmoa/MVP`).
- macOS או Linux (גם Windows עובד).

## הכנה – פעם אחת

### גרסה A – Electron

צריך [Node.js 22](https://nodejs.org). בטרמינל, מתוך תיקיית המאגר:

```
cd spikes/voice/electron
npm install
```

### גרסה B – מנוע Rust

צריך [Rust](https://rustup.rs) וגם כמה ספריות:

| מערכת | פקודה |
| --- | --- |
| macOS | `brew install opus pkg-config cmake` |
| Debian / Ubuntu | `sudo apt install build-essential pkg-config cmake libasound2-dev libopus-dev` |
| Fedora | `sudo dnf install gcc pkgconf cmake alsa-lib-devel opus-devel` |

ואז:

```
cd spikes/voice/str0m-peer
cargo build --release
```

בדיקה שהתקני השמע נראים (מיקרופונים ורמקולים; `(default)` = ברירת המחדל):

```
cargo run --release -- --list-devices
```

**רשות – עם ביטול הד ודיכוי רעשים** (צריך meson 1.1 ומעלה, ninja ו-clang;
ב-Debian 13 / Ubuntu 24.04: `sudo apt install meson ninja-build clang`, ב-macOS: `brew install meson ninja`):

```
cargo build --release --features apm
```

## הבדיקה – שלושה סבבים

בכל סבב כולם נכנסים לאותו חדר. כל אחד בתורו מדבר כ-30 שניות (סופרים, מקריאים
משהו), ואז כולם מדברים יחד כדקה, כמו במשחק. רושמים את התחושות (טופס למטה).

### סבב 1 – כולם בגרסה A

```
cd spikes/voice/electron
npm start -- --signal=ws://<השרת>:8080 --room=<החדר>-a --name=<השם שלך>
```

בחלון: בוחרים מיקרופון ורמקול/אוזניות, ולוחצים **הצטרפות**. בטבלה רואים כל
משתתף: RTT (זמן הלוך-חזור), jitter, אובדן חבילות ו-buffer. בסוף הסבב לוחצים
**שליחת דוח** ואז **יציאה**.

macOS ישאל אם לאפשר גישה למיקרופון – לאשר.

### סבב 2 – כולם בגרסה B

```
cd spikes/voice/str0m-peer
cargo run --release -- --signal ws://<השרת>:8080 --room <החדר>-b --name <השם שלך> --duration 300
```

`--input "<חלק משם המיקרופון>"` ו-`--output "<חלק משם הרמקול>"` בוחרים התקן
(למשל `--input Brio`). אחרי 300 שניות התוכנה שולחת דוח ויוצאת לבד.

macOS: בפעם הראשונה הטרמינל יבקש גישה למיקרופון. אם לא שומעים כלום – הגדרות
המערכת ← פרטיות ואבטחה ← מיקרופון ← לאפשר לטרמינל, ולהריץ שוב.

### סבב 3 – מעורב

שלושה בגרסה A ושלושה בגרסה B, באותו חדר (`<החדר>-c`). כך בודקים ששתי הגרסאות
מדברות זו עם זו.

## רק בלינוקס – בדיקת WebKitGTK (דקה)

Tauri מציג את הממשק בלינוקס עם WebKitGTK. בודקים מה הוא יודע לעשות בהפצה שלכם:

```
epiphany "file://$PWD/spikes/voice/web/index.html"
```

(`epiphany` = הדפדפן GNOME Web: `sudo apt install epiphany-browser`.)
בחלק **יכולות** בעמוד – לצלם מסך ולשלוח למורן. מעניין במיוחד הערך של
`RTCPeerConnection`: בבדיקות שלנו ב-Debian, Ubuntu, Fedora ו-Arch הוא `false`.

## מה שולחים למורן

לכל סבב:

1. איכות הקול, 1–5.
2. הרגשתם שיהוי (מדברים אחד על השני)? כן / לא.
3. שמעתם הד של עצמכם או של אחרים? מי?
4. רעשי רקע, קטיעות, מילים שנבלעו?
5. בחירת המיקרופון והרמקול עבדה?
6. המאוורר של המחשב התחיל לעבוד?

וגם: מערכת ההפעלה, סוג הרשת (סיב / סלולר / Wi-Fi), ואת שורת הפלט האחרונה של
גרסה B (מתחילה בשם שלכם ו-`cpu`).

## אם משהו לא עובד

| מה רואים | מה זה אומר |
| --- | --- |
| `cannot reach ws://...` | שרת האיתות לא זמין – לבדוק את הכתובת |
| משתתף מופיע בטבלה אבל אין קול | החיבור הישיר נכשל (NAT). לרשום מי ומאיזו רשת. גרסה B עוד לא יודעת לעבור דרך שרת TURN |
| `microphone: no input device` | אין מיקרופון ברירת מחדל – להוסיף `--input` |
| בגרסה A אין רשימת התקנים | עדיין לא אישרתם גישה למיקרופון – ללחוץ הצטרפות פעם אחת |

---

## למארגן (מורן)

שרת האיתות רץ על השרת שבו כבר רץ ה-relay. הכתובת והמפתחות לא נכנסים למאגר.

```
docker build -f spikes/voice/bench/Dockerfile.signal -t voice-signal spikes/voice
```

```
docker run -d --name voice-signal -p 8080:8080 -e STUN_URLS=stun:<PUBLIC_IP>:3478 -v ~/voice-reports:/results voice-signal
```

STUN בא מ-coturn שכבר מוגדר ב-`infra/` (`docker compose --profile voice up -d coturn`,
עם `TURN_SECRET`, `DOMAIN` ו-`PUBLIC_IP` בקובץ `.env`). לפתוח ב-firewall את 8080/tcp.
הדוחות נשמרים ב-`~/voice-reports/<חדר>/<שם>.json`, ומסכמים אותם עם
`node spikes/voice/bench/summarize.mjs` (מתוך תיקייה שבה `results` מצביע על הדוחות).

הערות:

- האיתות לא מוצפן (`ws://`) – מספיק לניסוי. הקול עצמו מוצפן מקצה לקצה (DTLS-SRTP) בשתי הגרסאות.
- גרסה A נפתחת מקובץ מקומי (`file://`), ולכן מותר לה להשתמש במיקרופון. בדפדפן רגיל, מעמוד `http://`, זה לא יעבוד.
- TURN עובד רק בגרסה A. ל-str0m אין לקוח TURN (ראו RESULTS.md).
