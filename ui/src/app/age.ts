export const MIN_AGE = 16;

/** Returns `YYYY-MM-DD` for a real calendar date in the past, else null. */
export function toBirthDate(day: string, month: string, year: string, now = new Date()): string | null {
  const d = Number(day);
  const m = Number(month);
  const y = Number(year);
  if (!Number.isInteger(d) || !Number.isInteger(m) || !Number.isInteger(y)) return null;
  if (y < 1900 || m < 1 || m > 12 || d < 1) return null;
  const date = new Date(Date.UTC(y, m - 1, d));
  if (date.getUTCDate() !== d || date.getUTCMonth() !== m - 1) return null; // e.g. 31/02
  if (date.getTime() > now.getTime()) return null;
  return `${String(y).padStart(4, '0')}-${String(m).padStart(2, '0')}-${String(d).padStart(2, '0')}`;
}

export function ageOn(birthDate: string, now = new Date()): number {
  const [y, m, d] = birthDate.split('-').map(Number);
  let age = now.getFullYear() - y;
  const month = now.getMonth() + 1;
  if (month < m || (month === m && now.getDate() < d)) age--;
  return age;
}

const REGIONS =
  'AD AE AF AG AL AM AO AR AT AU AZ BA BB BD BE BF BG BH BI BJ BN BO BR BS BT BW BY BZ CA CD CF CG CH CI CL CM CN CO CR CU CV CY CZ DE DJ DK DM DO DZ EC EE EG ER ES ET FI FJ FM FR GA GB GD GE GH GM GN GQ GR GT GW GY HN HR HT HU ID IE IL IN IQ IR IS IT JM JO JP KE KG KH KI KM KN KP KR KW KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME MG MH MK ML MM MN MR MT MU MV MW MX MY MZ NA NE NG NI NL NO NP NR NZ OM PA PE PG PH PK PL PS PT PW PY QA RO RS RU RW SA SB SC SD SE SG SI SK SL SM SN SO SR SS ST SV SY SZ TD TG TH TJ TL TM TN TO TR TT TV TW TZ UA UG US UY UZ VA VC VE VN VU WS YE ZA ZM ZW'.split(
    ' ',
  );

/** Country codes with names in the interface language, sorted for that language. */
export function countryOptions(locale: string): { code: string; name: string }[] {
  const names = new Intl.DisplayNames([locale], { type: 'region' });
  const collator = new Intl.Collator(locale);
  return REGIONS.map((code) => ({ code, name: names.of(code) ?? code })).sort((a, b) => collator.compare(a.name, b.name));
}

/** Country from the OS locale (e.g. he-IL → IL); falls back to Israel. */
export function detectCountry(languages: readonly string[] = typeof navigator !== 'undefined' ? navigator.languages : []): string {
  for (const l of languages) {
    try {
      const region = new Intl.Locale(l).maximize().region;
      if (region && REGIONS.includes(region)) return region;
    } catch {
      // ignore malformed tags
    }
  }
  return 'IL';
}
