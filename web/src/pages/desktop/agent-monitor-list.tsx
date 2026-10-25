import { Button } from 'antd';
import clsx from 'clsx';
import { useTranslation } from 'react-i18next';

import type { AgentMonitor } from '@/lib/agent-layout.ts';

/**
 * The target's monitors as the agent reads them, the board's marked, and
 * beside each other monitor "This is the board's", which tells the agent:
 * the Monitors menu's list and the settings' alike.
 */
export const AgentMonitorList = ({
  monitors,
  className,
  disabled,
  onChoose,
  testId
}: {
  monitors: AgentMonitor[];
  className: string;
  disabled: boolean;
  onChoose: (id: string) => void;
  testId?: string;
}) => {
  const { t } = useTranslation();
  return (
    <ul className={clsx('m-0 list-none p-0 text-xs', className)} data-testid={testId}>
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
              disabled={disabled}
              onClick={() => onChoose(monitor.id!)}
            >
              {t('settings.device.targetAgent.chooseBoard')}
            </Button>
          )}
        </li>
      ))}
    </ul>
  );
};
