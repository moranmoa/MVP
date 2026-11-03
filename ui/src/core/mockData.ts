// Fixed sample content for the mock core. Message bodies and names here are
// user content, not interface text, so they are not translated.

import type { AgeRestriction } from './types';

/** Short stand-in for the real 2048-word recovery list the core will use. */
export const WORDS = [
  'apple', 'river', 'stone', 'cloud', 'tiger', 'piano', 'lemon', 'rocket',
  'forest', 'candle', 'silver', 'garden', 'planet', 'window', 'butter', 'island',
  'mirror', 'orange', 'pencil', 'shadow', 'thunder', 'velvet', 'wagon', 'yellow',
  'anchor', 'bridge', 'castle', 'dragon', 'engine', 'falcon', 'guitar', 'harbor',
  'jungle', 'kettle', 'ladder', 'magnet', 'needle', 'oyster', 'parrot', 'quartz',
  'rabbit', 'saddle', 'tunnel', 'umbrella', 'violin', 'walnut', 'zebra', 'basket',
  'cactus', 'desert', 'feather', 'glacier', 'helmet', 'icicle', 'jacket', 'kitten',
  'lantern', 'meadow', 'nickel', 'orchid', 'pepper', 'radar', 'sunset', 'trumpet',
];

export const PEOPLE = [
  'נועה', 'Daniel', 'יובל', 'מאיה', 'Omer', 'תמר', 'איתי', 'Shira', 'אלון', 'רוני',
  'Lior', 'עדי', 'גל', 'Noam', 'הילה', 'Yonatan', 'שקד', 'עומרי', 'Tal', 'אביב',
  'Eden', 'ליאור', 'דנה', 'Itamar', 'ספיר', 'Nadav', 'אורי', 'Maya_K', 'בר', 'Roy',
];

export const BODIES = [
  'מישהו בא לסיבוב הערב?',
  'אני בפנים, עוד עשר דקות',
  'ראיתם את העדכון החדש? שינו את כל המפה',
  'GG לכולם 🎉',
  'anyone up for ranked tonight?',
  'patch notes are out, the new map looks sick',
  'brb',
  'מי מארח היום?',
  'יש למישהו המלצה על מיקרופון טוב בפחות מ-300 ₪?',
  'lol זה היה מטורף',
  'אני על RTX 4070 וזה רץ חלק ב-144fps',
  'שלחתי את הקליפ בערוץ clips',
  'check out this clip, the last round was insane',
  'מחר ב-21:00 טורניר פנימי, מי נרשם?',
  'אפשר להוסיף אותי לקבוצה?',
  'the server was laggy for me yesterday, anyone else?',
  'אצלי הכל עבד חלק, אולי זה הראוטר?',
  'סיימתי את הקמפיין סוף סוף 😅',
  'מי שרוצה ללמוד את המפה החדשה – אני מסביר הערב בקול',
  'ok see you at 9',
  'חשבתי על זה עוד קצת: אם נחלק את הקבוצה לשתיים, כל אחת תתאמן על תפקיד אחר, ובסוף השבוע נעשה משחק משותף ונראה מה עובד. ככה גם החדשים מקבלים זמן משחק אמיתי ולא רק יושבים ומחכים.',
  'version 2.4.1 fixed the crash on alt-tab for me',
  'תודה! עזר מאוד',
  'מישהו יודע איך משנים את הרזולוציה בלי להפעיל מחדש?',
];

export interface ServerSeed {
  id: string;
  name: string;
  color: string;
  ageRestriction: AgeRestriction;
  /** Index into PEOPLE, or -1 for "me". */
  owner: number;
  /** How many of PEOPLE are members, and how many of those are online. */
  members: number;
  online: number;
  channels: { id: string; name: string; adult?: boolean; count: number }[];
}

export const DEMO_SERVERS: ServerSeed[] = [
  {
    id: 's-gamers',
    name: 'גיימרים ישראל',
    color: '#5865f2',
    ageRestriction: '16+',
    owner: -1,
    members: 30,
    online: 12,
    channels: [
      { id: 'c-general', name: 'כללי', count: 100_000 },
      { id: 'c-lfg', name: 'מחפשים-קבוצה', count: 2_000 },
      { id: 'c-clips', name: 'clips', count: 500 },
      { id: 'c-adult', name: 'אחרי-חצות', adult: true, count: 200 },
    ],
  },
  {
    id: 's-mc',
    name: 'Minecraft IL',
    color: '#2f9e44',
    ageRestriction: 'none',
    owner: 1,
    members: 18,
    online: 5,
    channels: [
      { id: 'c-mc-general', name: 'general', count: 5_000 },
      { id: 'c-mc-builds', name: 'builds', count: 300 },
    ],
  },
  {
    id: 's-friends',
    name: 'החבר׳ה',
    color: '#e8590c',
    ageRestriction: '18+',
    owner: 3,
    members: 6,
    online: 0,
    channels: [{ id: 'c-fr-general', name: 'כללי', count: 40 }],
  },
];

/** Joined through an invite link in the demo. */
export const JOINABLE_SERVER: ServerSeed = {
  id: 's-valorant',
  name: 'Valorant ישראל',
  color: '#d6336c',
  ageRestriction: '16+',
  owner: 7,
  members: 22,
  online: 9,
  channels: [
    { id: 'c-val-general', name: 'כללי', count: 1_200 },
    { id: 'c-val-lfg', name: 'lfg', count: 150 },
  ],
};

export const AVATAR_COLORS = [
  '#5865f2', '#2f9e44', '#e8590c', '#d6336c', '#1098ad', '#7048e8', '#f08c00', '#0ca678',
];
