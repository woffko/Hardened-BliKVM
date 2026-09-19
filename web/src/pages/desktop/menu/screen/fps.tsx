import { useRef, useState } from 'react';
import { CheckOutlined, CloseOutlined } from '@ant-design/icons';
import { Button, InputNumber } from 'antd';
import { CheckIcon, ScanBarcodeIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { updateScreen } from '@/api/vm';
import { setFps as setCookie } from '@/lib/localstorage';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

const fpsList = [
  { key: 60, label: '60Hz' },
  { key: 30, label: '30Hz' },
  { key: 24, label: '24Hz' }
];

const defaultFps = [60, 30, 24];

type FpsProps = {
  fps: number;
  setFps: (fps: number) => void;
  // The measured rate the capture bridge delivers; undefined when unknown.
  capturedFps?: number;
  deliveredWidth?: number;
  deliveredHeight?: number;
  queuedFps?: number;
};

export const Fps = ({
  fps,
  setFps,
  capturedFps,
  deliveredWidth,
  deliveredHeight,
  queuedFps
}: FpsProps) => {
  const { t } = useTranslation();
  const [isCustomize, setIsCustomize] = useState(false);
  const customizeRef = useRef(0);

  function showCustomize() {
    customizeRef.current = fps;
    setIsCustomize(true);
  }

  function onChange(value: any) {
    const num = Number(value);
    if (num > 0 && num <= 60) {
      customizeRef.current = num;
    }
  }

  async function update(value: number) {
    if (isCustomize && customizeRef.current === fps) {
      setIsCustomize(false);
      return;
    }

    const rsp = await updateScreen('fps', value);
    if (rsp.code !== 0) {
      return;
    }

    setFps(value);
    setCookie(value);
    if (isCustomize) {
      setIsCustomize(false);
    }
  }

  const content = (
    <>
      {/* default fps list */}
      {fpsList.map((item) => (
        <div
          key={item.key}
          className="flex cursor-pointer select-none items-center rounded py-1.5 pl-1 hover:bg-neutral-700/70"
          onClick={() => update(item.key)}
        >
          <div className="flex h-[14px] w-[20px] items-end text-blue-500">
            {item.key === fps && <CheckIcon size={14} />}
          </div>
          <span>{item.label}</span>
        </div>
      ))}

      {/* customize fps */}
      <div
        className="flex cursor-pointer select-none items-center rounded py-1.5 pl-1 pr-5 hover:bg-neutral-700/70"
        onClick={showCustomize}
      >
        {defaultFps.includes(fps) ? (
          <>
            <div className="flex h-[14px] w-[20px] items-end"></div>
            <span>{t('screen.customizeFps')}</span>
          </>
        ) : (
          <>
            <div className="flex h-[14px] w-[20px] items-end text-blue-500">
              <CheckIcon size={14} />
            </div>
            <span>Customize</span>
            <span className="text-xs">{`(${fps}Hz)`}</span>
          </>
        )}
      </div>

      {typeof deliveredWidth === 'number' && typeof deliveredHeight === 'number' && (
        <div className="pl-1 pt-1 text-xs text-neutral-400">
          {t('screen.deliveredFrame', {
            width: deliveredWidth,
            height: deliveredHeight
          })}
          {typeof capturedFps === 'number' &&
            capturedFps !== fps &&
            ` · ${t('screen.deliveredFps', { value: capturedFps })}`}
          {typeof queuedFps === 'number' &&
            ` · ${t('screen.deliveredQueued', { value: queuedFps })}`}
        </div>
      )}

      {isCustomize && (
        <div className="flex w-[140px] items-center space-x-1 py-1">
          <InputNumber<number> defaultValue={fps} min={1} max={60} onChange={onChange} />
          <Button
            size="small"
            icon={<CheckOutlined />}
            onClick={() => update(customizeRef.current)}
          />
          <Button size="small" icon={<CloseOutlined />} onClick={() => setIsCustomize(false)} />
        </div>
      )}
    </>
  );

  return (
    <MenuSubmenu icon={<ScanBarcodeIcon size={18} />} label={t('screen.fps')} content={content} />
  );
};
