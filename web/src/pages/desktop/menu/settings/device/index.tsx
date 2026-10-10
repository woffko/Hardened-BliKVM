import { Divider } from 'antd';
import { useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';

import { boardFeatureIsWritable, supportsBoardFeature } from '@/lib/capabilities.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';

import { Advanced } from './advanced';
import { HidOnly } from './hid-only.tsx';
import { Mdns } from './mdns.tsx';
import { MouseJiggler } from './mouse-jiggler.tsx';
import { Oled } from './oled.tsx';
import { Reboot } from './reboot.tsx';
import { SessionLock } from './session-lock.tsx';
import { Ssh } from './ssh.tsx';
import { VirtualDevices } from './virtual-devices.tsx';

export const Device = () => {
  const { t } = useTranslation();
  const capabilities = useAtomValue(capabilitiesAtom);
  const has = (feature: string) => supportsBoardFeature(capabilities, feature);

  return (
    <>
      <div className="text-base">{t('settings.device.title')}</div>
      <Divider className="opacity-50" />

      <div className="flex flex-col space-y-8">
        {has('device_ssh') && <Ssh />}
        {has('device_mdns') && (
          <Mdns readOnly={!boardFeatureIsWritable(capabilities, 'device_mdns')} />
        )}
        {has('device_hid_only') && (
          <HidOnly readOnly={!boardFeatureIsWritable(capabilities, 'device_hid_only')} />
        )}
        {has('device_virtual_devices') && <VirtualDevices />}
        {has('device_oled') && (
          <Oled readOnly={!boardFeatureIsWritable(capabilities, 'device_oled')} />
        )}
        {has('device_mouse_jiggler') && (
          <MouseJiggler readOnly={!boardFeatureIsWritable(capabilities, 'device_mouse_jiggler')} />
        )}
        {has('device_session_lock') && (
          <SessionLock readOnly={!boardFeatureIsWritable(capabilities, 'device_session_lock')} />
        )}
        {has('device_advanced') && <Advanced />}
      </div>

      {has('device_reboot') && (
        <>
          <Divider className="opacity-50" />
          <Reboot />
        </>
      )}
    </>
  );
};
