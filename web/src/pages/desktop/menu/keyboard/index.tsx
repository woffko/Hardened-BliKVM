import { useAtomValue } from 'jotai';
import { KeyboardIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { agentKeyboardAtom } from '@/jotai/target-agent.ts';
import { MenuItem } from '@/components/menu-item.tsx';

import { AgentKeyboard } from './agent.tsx';
import { LeaderKey } from './leader-key.tsx';
import { LocalKeyboard } from './local-keyboard.tsx';
import { Paste } from './paste.tsx';
import { Shortcuts } from './shortcuts';
import { VirtualKeyboard } from './virtual-keyboard.tsx';

export const Keyboard = () => {
  const { t } = useTranslation();
  const layout = useAtomValue(agentKeyboardAtom)?.current ?? null;

  const content = (
    <div className="flex flex-col space-y-1">
      {layout && (
        <div className="px-3 py-1 text-xs text-neutral-400" data-testid="target-layout">
          {t('settings.device.targetAgent.targetLayout', { name: layout.name })}
        </div>
      )}
      <AgentKeyboard />
      <Paste />
      <LocalKeyboard />
      <VirtualKeyboard />
      <Shortcuts />
      <LeaderKey />
    </div>
  );

  return (
    <MenuItem title={t('keyboard.title')} icon={<KeyboardIcon size={18} />} content={content} />
  );
};
