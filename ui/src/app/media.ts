import { useSyncExternalStore } from 'react';

/** Phone width. In CSS media queries rem is always 16px, whatever the UI scale. */
export const PHONE_QUERY = '(max-width: 40rem)';

/** True while the media query matches; follows window resizes. */
export function useMediaQuery(query: string): boolean {
  return useSyncExternalStore(
    (onChange) => {
      if (typeof matchMedia !== 'function') return () => {};
      const mq = matchMedia(query);
      mq.addEventListener('change', onChange);
      return () => mq.removeEventListener('change', onChange);
    },
    () => typeof matchMedia === 'function' && matchMedia(query).matches,
  );
}
