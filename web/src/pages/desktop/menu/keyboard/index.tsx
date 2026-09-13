import { useAtomValue } from 'jotai';
import { KeyboardIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { supportsRestrictedControls } from '@/lib/capabilities.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { MenuItem } from '@/components/menu-item.tsx';

import { LeaderKey } from './leader-key.tsx';
import { LocalKeyboard } from './local-keyboard.tsx';
import { Paste } from './paste.tsx';
import { Shortcuts } from './shortcuts';
import { VirtualKeyboard } from './virtual-keyboard.tsx';

export const Keyboard = () => {
  const { t } = useTranslation();
  const manifest = useAtomValue(capabilitiesAtom);
  const restricted = supportsRestrictedControls(manifest);

  const content = (
    <div className="flex flex-col space-y-1">
      <Paste />
      <LocalKeyboard />
      <VirtualKeyboard />
      {!restricted && <Shortcuts />}
      {!restricted && <LeaderKey />}
    </div>
  );

  return (
    <MenuItem title={t('keyboard.title')} icon={<KeyboardIcon size={18} />} content={content} />
  );
};
