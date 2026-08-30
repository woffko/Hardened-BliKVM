import { useAtomValue } from 'jotai';

import { mouseModeAtom } from '@/jotai/mouse.ts';
import { useIsTouchLayout } from '@/hooks/useResponsiveLayout.ts';

import { Absolute } from './absolute.tsx';
import { MobileCursorOverlay } from './mobile-cursor-overlay.tsx';
import { Relative } from './relative.tsx';

export const Mouse = () => {
  const isTouchLayout = useIsTouchLayout();
  const mouseMode = useAtomValue(mouseModeAtom);

  if (isTouchLayout) {
    return (
      <>
        <Absolute />
        <MobileCursorOverlay />
      </>
    );
  }

  return <>{mouseMode === 'relative' ? <Relative /> : <Absolute />}</>;
};
