import { useRef } from 'react';
import { message } from 'antd';
import { CheckIcon, SquareActivityIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { updateScreen } from '@/api/vm';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

// The target bitrates the board's VP8 and H.264 encoders offer, in kbit/s.
export const WEBRTC_QUALITY_LEVELS = [
  { kbps: 1500, label: 'screen.qualityLow' },
  { kbps: 2500, label: 'screen.qualityMedium' },
  { kbps: 5000, label: 'screen.qualityHigh' },
  { kbps: 8000, label: 'screen.qualityVeryHigh' }
] as const;

type WebrtcQualityProps = {
  kbps: number;
  refresh: () => Promise<{ webrtcKbps?: number }>;
};

/** The WebRTC encoders' bitrate: applied by the running encoders, so the
 * picture goes on without a reconnect. */
export const WebrtcQuality = ({ kbps, refresh }: WebrtcQualityProps) => {
  const { t } = useTranslation();
  const updating = useRef(false);

  async function update(value: number) {
    if (updating.current || value === kbps) return;
    updating.current = true;
    try {
      const rsp = await updateScreen('webrtc_kbps', value);
      if (rsp.code !== 0 || rsp.data?.applied !== true || rsp.data?.rejection != null)
        throw new Error(rsp.data?.rejection || 'Quality update was refused');
      if ((await refresh()).webrtcKbps !== value)
        throw new Error('Quality read-back did not match the request');
    } catch (err) {
      message.error(err instanceof Error ? err.message : 'Unable to update quality');
    } finally {
      updating.current = false;
    }
  }

  const content = (
    <>
      {WEBRTC_QUALITY_LEVELS.map((level) => (
        <div
          key={level.kbps}
          data-webrtc-kbps={level.kbps}
          className="flex h-[30px] cursor-pointer select-none items-center rounded pl-1 pr-5 hover:bg-neutral-700/70"
          onClick={() => update(level.kbps)}
        >
          <div className="flex h-[14px] w-[20px] items-end text-blue-500">
            {level.kbps === kbps && <CheckIcon size={14} />}
          </div>
          <span className="flex whitespace-nowrap">
            {t(level.label)}
            <span className="pl-2 text-neutral-500">
              {t('screen.megabitsPerSecond', { value: (level.kbps / 1000).toLocaleString() })}
            </span>
          </span>
        </div>
      ))}
    </>
  );

  return (
    <MenuSubmenu
      icon={<SquareActivityIcon size={18} />}
      label={t('screen.quality')}
      content={content}
    />
  );
};
