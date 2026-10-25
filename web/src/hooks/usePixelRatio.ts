import { useEffect, useState } from 'react';

/**
 * The display's device pixels per CSS pixel, kept current: it changes with
 * the browser's zoom and when the window moves to another display, which a
 * media query on the current ratio reports.
 */
export function usePixelRatio(): number {
  const [ratio, setRatio] = useState(() =>
    typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1
  );

  useEffect(() => {
    let query: MediaQueryList | null = null;
    const update = () => {
      query?.removeEventListener('change', update);
      const current = window.devicePixelRatio || 1;
      setRatio(current);
      query = window.matchMedia(`(resolution: ${current}dppx)`);
      query.addEventListener('change', update);
    };
    update();
    return () => query?.removeEventListener('change', update);
  }, []);

  return ratio;
}
