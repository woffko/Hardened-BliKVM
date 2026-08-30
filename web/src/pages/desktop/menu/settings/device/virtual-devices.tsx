import { useEffect, useState } from 'react';
import { Switch } from 'antd';
import { useTranslation } from 'react-i18next';

import { getHidMode, getUsbWakeup, setUsbWakeup } from '@/api/hid.ts';
import * as api from '@/api/virtual-device.ts';

export const VirtualDevices = () => {
  const { t } = useTranslation();

  const [isHidOnlyMode, setIsHidOnlyMode] = useState(false);
  const [isUsbWakeupEnabled, setIsUsbWakeupEnabled] = useState(false);
  const [isDiskEnabled, setIsDiskEnabled] = useState(false);
  const [isNetworkEnabled, setIsNetworkEnabled] = useState(false);
  const [loading, setLoading] = useState<'' | 'disk' | 'network' | 'wakeup'>('');

  useEffect(() => {
    getHidOnlyMode();
    getVirtualDevice();
    getWakeup();
  }, []);

  async function getHidOnlyMode() {
    try {
      const rsp = await getHidMode();
      if (rsp.code !== 0) {
        console.log(rsp.msg);
        return;
      }
      setIsHidOnlyMode(rsp.data.mode === 'hid-only');
    } catch (err) {
      console.log(err);
    }
  }

  async function getVirtualDevice() {
    try {
      const rsp = await api.getVirtualDevice();
      if (rsp.code !== 0) {
        console.log(rsp.msg);
        return;
      }

      setIsDiskEnabled(rsp.data.disk);
      setIsNetworkEnabled(rsp.data.network);
    } catch (err) {
      console.log(err);
    }
  }

  async function getWakeup() {
    try {
      const rsp = await getUsbWakeup();
      if (rsp.code !== 0) {
        console.log(rsp.msg);
        return;
      }
      setIsUsbWakeupEnabled(rsp.data.enabled);
    } catch (err) {
      console.log(err);
    }
  }

  async function update(device: 'disk' | 'network') {
    if (loading) return;
    setLoading(device);

    try {
      const rsp = await api.updateVirtualDevice(device);
      if (rsp.code !== 0) {
        console.log(rsp.msg);
        return;
      }

      await getVirtualDevice();
    } catch (err) {
      console.log(err);
    } finally {
      setLoading('');
    }
  }

  async function updateWakeup(enabled: boolean) {
    if (loading) return;
    setLoading('wakeup');

    try {
      const rsp = await setUsbWakeup(enabled);
      if (rsp.code !== 0) {
        console.log(rsp.msg);
        return;
      }
      setIsUsbWakeupEnabled(rsp.data.enabled);
    } catch (err) {
      console.log(err);
    } finally {
      setLoading('');
    }
  }

  const usbWakeup = (
    <div className="flex items-center justify-between">
      <div className="flex flex-col space-y-1">
        <span>{t('settings.device.usbWakeup')}</span>
        <span className="text-xs text-neutral-500">{t('settings.device.usbWakeupDesc')}</span>
      </div>

      <Switch checked={isUsbWakeupEnabled} loading={loading === 'wakeup'} onChange={updateWakeup} />
    </div>
  );

  if (isHidOnlyMode) {
    return (
      <>
        <div className="flex items-center justify-between space-x-10">
          <div className="flex flex-col space-y-1">
            <span>{t('settings.device.hidOnly')}</span>
            <span className="text-xs text-neutral-500">{t('settings.device.hidOnlyDesc')}</span>
          </div>

          <Switch checked={true} disabled={true} />
        </div>
        {usbWakeup}
      </>
    );
  }

  return (
    <>
      {/* Virtual Disk */}
      <div className="flex items-center justify-between">
        <div className="flex flex-col space-y-1">
          <span>{t('settings.device.disk')}</span>
          <span className="text-xs text-neutral-500">{t('settings.device.diskDesc')}</span>
        </div>

        <Switch
          checked={isDiskEnabled}
          loading={loading === 'disk'}
          onChange={() => update('disk')}
        />
      </div>

      {/* Virtual Network */}
      <div className="flex items-center justify-between">
        <div className="flex flex-col space-y-1">
          <span>{t('settings.device.network')}</span>
          <span className="text-xs text-neutral-500">{t('settings.device.networkDesc')}</span>
        </div>

        <Switch
          checked={isNetworkEnabled}
          loading={loading === 'network'}
          onChange={() => update('network')}
        />
      </div>

      {usbWakeup}
    </>
  );
};
