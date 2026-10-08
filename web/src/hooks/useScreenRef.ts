import type { MutableRefObject } from 'react';
import { useCallback } from 'react';
import { useSetAtom } from 'jotai';

import { screenElementAtom } from '@/jotai/screen.ts';

/**
 * A ref for a view's `#screen` element that also tells the page which
 * element it is, so that input binds to the element now on the page.
 */
export function useScreenRef<T extends HTMLElement>(ref?: MutableRefObject<T | null>) {
  const setScreen = useSetAtom(screenElementAtom);
  return useCallback(
    (node: T | null) => {
      if (ref) ref.current = node;
      setScreen(node);
    },
    [ref, setScreen]
  );
}
