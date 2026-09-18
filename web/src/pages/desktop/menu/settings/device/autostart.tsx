import { useEffect, useState } from 'react';
import { useSetAtom } from 'jotai';
import { NetworkIcon, PlayIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/autostart.ts';
import { settingsTabAtom } from '@/jotai/settings.ts';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

/// Autostart scripts stay owner-only: the hardened server lists what the
/// operator placed, and uploads happen over the owner's own access.
export const Autostart = () => {
  const { t } = useTranslation();

  const [files, setFiles] = useState<string[]>([]);
  const setSettingsTab = useSetAtom(settingsTabAtom);

  useEffect(() => {
    api
      .getAutostart()
      .then((rsp) => {
        if (rsp.code === 0) {
          setFiles(rsp.data?.files ?? []);
        }
      })
      .catch(() => setFiles([]));
  }, []);

  const content = (
    <div className="flex flex-col space-y-1">
      {files.length === 0 && (
        <div className="px-1 text-xs text-neutral-500">{t('settings.device.autostart.empty')}</div>
      )}
      {files.map((file) => (
        <div key={file} className="flex h-[30px] select-none items-center rounded pl-1 pr-5">
          <div className="flex h-[14px] w-[20px] items-end text-blue-500">
            <PlayIcon size={12} />
          </div>
          <span className="truncate">{file}</span>
        </div>
      ))}
      <div
        className="flex h-[30px] cursor-pointer select-none items-center rounded pl-1 pr-5 hover:bg-neutral-700/70"
        onClick={() => setSettingsTab('system')}
      >
        <div className="flex h-[14px] w-[20px] items-end text-blue-500">
          <NetworkIcon size={12} />
        </div>
        <span>{t('settings.device.autostart.network')}</span>
      </div>
    </div>
  );

  return (
    <MenuSubmenu
      icon={<PlayIcon size={16} />}
      label={t('settings.device.autostart.title')}
      content={content}
    />
  );
};
