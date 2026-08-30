import { useAtomValue } from 'jotai';
import { useMediaQuery } from 'react-responsive';

import { layoutModeAtom } from '@/jotai/settings.ts';

const NARROW_LAYOUT_MAX_WIDTH = 849;
const TOUCH_TABLET_MAX_WIDTH = 1366;

export function useIsTouchLayout() {
  const layoutMode = useAtomValue(layoutModeAtom);
  const isNarrow = useMediaQuery({ maxWidth: NARROW_LAYOUT_MAX_WIDTH });
  const isTabletOrSmaller = useMediaQuery({ maxWidth: TOUCH_TABLET_MAX_WIDTH });
  const hasCoarsePointer = useMediaQuery({ query: '(pointer: coarse)' });
  const hasNoHover = useMediaQuery({ query: '(hover: none)' });

  return (
    layoutMode === 'mobile' || isNarrow || (isTabletOrSmaller && (hasCoarsePointer || hasNoHover))
  );
}

export function useIsDesktopLayout() {
  return !useIsTouchLayout();
}
