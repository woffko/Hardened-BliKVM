import { useEffect, useLayoutEffect, useState } from 'react';
import { router } from '@/router';
import { ConfigProvider, theme } from 'antd';
import { useAtomValue } from 'jotai';
import { RouterProvider } from 'react-router-dom';

import { ACCENTS, applyTheme, LIGHT_QUERY, resolveTheme } from '@/lib/theme.ts';
import { accentAtom, themeModeAtom } from '@/jotai/settings.ts';

/** antd and the document follow the theme and accent chosen in this browser
 * (Settings > Appearance), and the system's theme for "System". */
export const ThemedApp = () => {
  const themeMode = useAtomValue(themeModeAtom);
  const accent = useAtomValue(accentAtom);
  const [systemLight, setSystemLight] = useState(() => window.matchMedia(LIGHT_QUERY).matches);

  useEffect(() => {
    const query = window.matchMedia(LIGHT_QUERY);
    const changed = () => setSystemLight(query.matches);
    query.addEventListener('change', changed);
    return () => query.removeEventListener('change', changed);
  }, []);

  const resolved = resolveTheme(themeMode, systemLight);
  useLayoutEffect(() => applyTheme(resolved, accent), [resolved, accent]);

  const primary = ACCENTS[accent]?.primary;
  const themeConfig = {
    algorithm: resolved === 'light' ? theme.defaultAlgorithm : theme.darkAlgorithm,
    token: primary ? { colorPrimary: primary } : undefined,
    components: {
      Collapse: {
        headerPadding: 0,
        contentPadding: 0
      }
    }
  };

  return (
    <ConfigProvider theme={themeConfig}>
      <RouterProvider router={router} />
    </ConfigProvider>
  );
};
