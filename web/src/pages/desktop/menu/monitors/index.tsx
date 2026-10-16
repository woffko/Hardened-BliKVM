import { useState } from 'react';
import { Button } from 'antd';
import clsx from 'clsx';
import { useAtomValue } from 'jotai';
import { MonitorCogIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { DisplayMode } from '@/api/target-agent.ts';
import { displaysNow } from '@/lib/agent-layout.ts';
import { agentMonitorsAtom, targetAgentAtom } from '@/jotai/target-agent.ts';
import { readAgentMonitors, useAgentDisplays } from '@/hooks/useAgentDisplays.ts';
import { useFocusConfirmation } from '@/hooks/useFocusConfirmation.ts';
import { useHidLease } from '@/hooks/useHidLease.ts';
import { MenuItem } from '@/components/menu-item.tsx';

/** Windows' own choices, in its order, with the board's monitor for "show only on". */
const MODES: DisplayMode[] = ['duplicate', 'extend', 'board_only', 'others_only'];

/**
 * The target's monitors while the agent is connected
 * (docs/design/target-agent-v1.md): the mode they are in, as the agent
 * reads them; Windows' choices to change it, put back by the agent unless
 * kept; and which monitor is the board's, after which the absolute mouse
 * keeps to it in every mode.
 */
export const Monitors = () => {
  const { t } = useTranslation();
  const connected = Boolean(useAtomValue(targetAgentAtom)?.agent);
  const monitors = useAtomValue(agentMonitorsAtom);
  const displays = useAgentDisplays();
  const { confirmed } = useFocusConfirmation();
  const { hold, drained } = useHidLease();
  const [readError, setReadError] = useState('');

  if (!connected) return null;

  // Opened under remote control: the monitors are read again, under the
  // keyboard's and mouse's lease, taken without input if no input took it
  // yet; a read that fails says why.
  function opened(open: boolean) {
    if (!open || !confirmed) return;
    setReadError('');
    hold();
    void drained()
      .then(() => readAgentMonitors())
      .catch((err: unknown) => setReadError(err instanceof Error ? err.message : String(err)));
  }

  const now = monitors ? displaysNow(monitors) : 'unknown';
  const boardKnown = Boolean(monitors?.some((monitor) => monitor.board));

  const content = (
    <div
      className="flex w-[300px] flex-col space-y-2 px-3 py-2 text-sm"
      data-testid="agent-monitors"
    >
      {!monitors && (
        <span className="text-xs text-neutral-500" data-testid="agent-monitors-unread">
          {confirmed
            ? t('settings.device.targetAgent.monitorsReading')
            : t('settings.device.targetAgent.monitorsNeedControl')}
        </span>
      )}
      {monitors && (
        <>
          <span className="text-xs text-neutral-400" data-testid="agent-monitors-now">
            {t('settings.device.targetAgent.displays.now', {
              mode:
                now === 'unknown'
                  ? t('settings.device.targetAgent.displays.unknown')
                  : t(`settings.device.targetAgent.displays.${now}`)
            })}
          </span>
          <ul className="m-0 flex list-none flex-col space-y-1 p-0 text-xs text-neutral-300">
            {monitors.map((monitor) => (
              <li
                key={monitor.id || `${monitor.x},${monitor.y}`}
                className="flex items-center justify-between gap-2"
              >
                <span className="min-w-0 flex-1">
                  {t('settings.device.targetAgent.monitor', {
                    name: monitor.name || monitor.maker,
                    width: monitor.width,
                    height: monitor.height,
                    x: monitor.x,
                    y: monitor.y
                  })}
                  {monitor.board ? ` — ${t('settings.device.targetAgent.boards')}` : ''}
                </span>
                {!monitor.board && monitor.id && (
                  <Button
                    size="small"
                    type="link"
                    className="h-auto shrink-0 px-0 text-xs"
                    disabled={displays.busy}
                    onClick={() => void displays.chooseBoard(monitor.id!)}
                  >
                    {t('settings.device.targetAgent.chooseBoard')}
                  </Button>
                )}
              </li>
            ))}
          </ul>
          {!boardKnown && (
            <span className="text-xs text-neutral-500">
              {t('settings.device.targetAgent.chooseBoardHint')}
            </span>
          )}
          <div className="flex flex-wrap gap-1" data-testid="agent-monitors-modes">
            {MODES.map((mode) => (
              <Button
                key={mode}
                size="small"
                type={now === mode ? 'primary' : 'default'}
                className={clsx(now === mode && 'pointer-events-none')}
                disabled={
                  displays.busy ||
                  displays.keeping ||
                  ((mode === 'board_only' || mode === 'others_only') && !boardKnown)
                }
                onClick={() => void displays.changeDisplays(mode)}
              >
                {t(`settings.device.targetAgent.displays.${mode}`)}
              </Button>
            ))}
          </div>
        </>
      )}
      {(readError || displays.error) && (
        <span role="alert" className="text-xs text-red-400">
          {readError || displays.error}
        </span>
      )}
    </div>
  );

  return (
    <MenuItem
      title={t('settings.device.targetAgent.displays.title')}
      icon={<MonitorCogIcon size={18} />}
      content={content}
      onOpenChange={opened}
    />
  );
};
