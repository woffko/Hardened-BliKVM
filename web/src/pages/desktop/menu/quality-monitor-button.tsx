import { Tooltip } from 'antd';
import clsx from 'clsx';
import { useAtom } from 'jotai';
import { useTranslation } from 'react-i18next';

import { qualityMonitorAtom } from '@/jotai/quality-monitor.ts';

/** QM: opens and closes the quality monitor. */
export const QualityMonitorButton = () => {
  const { t } = useTranslation();
  const [layout, setLayout] = useAtom(qualityMonitorAtom);

  return (
    <Tooltip title={t('qualityMonitor.title')} placement="bottom" mouseEnterDelay={0.6}>
      <div
        role="button"
        data-testid="quality-monitor-toggle"
        aria-pressed={layout.open}
        className={clsx(
          'flex h-[30px] min-w-[30px] cursor-pointer select-none items-center justify-center rounded px-1 text-[13px] font-semibold',
          layout.open
            ? 'bg-neutral-700/80 text-white'
            : 'text-neutral-300 hover:bg-neutral-700/80 hover:text-white'
        )}
        onClick={() => setLayout({ open: !layout.open })}
      >
        QM
      </div>
    </Tooltip>
  );
};
