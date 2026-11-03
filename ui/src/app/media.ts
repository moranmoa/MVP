import { useSyncExternalStore } from 'react';

/**
 * Phone width at the given UI scale. In media queries rem is always 16px, so the
 * scale is applied here; the CSS uses a container query on #root, which sees it.
 */
export function phoneQuery(uiScale: number): string {
  return `(max-width: ${40 * uiScale}rem)`;
}

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
