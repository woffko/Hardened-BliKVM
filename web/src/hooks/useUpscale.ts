import { useState } from 'react';
import { useAtomValue } from 'jotai';

import { fitFor, type Fit, type Sharpness } from '@/lib/upscale/settings.ts';
import { drawsWithGpu } from '@/lib/vp8-range.ts';
import { upscaleAtom, upscaleStoppedAtom } from '@/jotai/screen.ts';

/**
 * How the views draw the picture now: with the sharpness of the setting
 * when it is Auto, the browser draws with a GPU and upscaling has not
 * stopped on this page load; otherwise `off`, the plain path.
 */
export function useUpscale(): Sharpness | 'off' {
  const setting = useAtomValue(upscaleAtom);
  const stopped = useAtomValue(upscaleStoppedAtom);
  const [gpu] = useState(() => drawsWithGpu());
  return setting.mode === 'auto' && gpu && !stopped ? setting.sharpness : 'off';
}

/**
 * How the views draw the picture now, and how large: upscaled pictures fill
 * the view, or are shown at the target's resolution, beyond their own size.
 */
export function useUpscaleView(): { upscale: Sharpness | 'off'; fit: Fit } {
  const setting = useAtomValue(upscaleAtom);
  const upscale = useUpscale();
  return { upscale, fit: fitFor(setting, upscale !== 'off') };
}
