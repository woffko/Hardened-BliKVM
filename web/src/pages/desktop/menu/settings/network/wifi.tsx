import { useCallback, useEffect, useState } from 'react';
import { LockOutlined, WifiOutlined } from '@ant-design/icons';
import { Button, Input, Modal, Switch } from 'antd';
import { WifiIcon, WifiPenIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/network.ts';

export const Wifi = ({ readOnly = false }: { readOnly?: boolean }) => {
  const { t } = useTranslation();

  const [isSupported, setIsSupported] = useState(false);
  const [isAPMode, setIsAPMode] = useState(false);
  const [connectedWiFi, setConnectedWifi] = useState('');

  const [isModalOpen, setIsModalOpen] = useState(false);
  const [ssid, setSsid] = useState('');
  const [password, setPassword] = useState('');
  const [status, setStatus] = useState<'' | 'connecting' | 'disconnecting'>('');
  const [message, setMessage] = useState('');

  const getWiFi = useCallback(async () => {
    try {
      const rsp = await api.getWiFi();
      if (rsp.code !== 0) {
        throw new Error('Failed to read Wi-Fi state');
      }

      const data = rsp.data;
      const available = data?.available ?? data?.supported;
      if (
        typeof available !== 'boolean' ||
        typeof data?.connected !== 'boolean' ||
        typeof data?.ssid !== 'string'
      )
        throw new Error('Invalid Wi-Fi state');
      setIsSupported(available);
      setIsAPMode(!!rsp.data?.apMode);

      if (rsp.data?.connected && rsp.data?.ssid) {
        setConnectedWifi(rsp.data.ssid);
      } else {
        setConnectedWifi('');
      }
      return data;
    } catch {
      setMessage(t('settings.network.wifi.failed'));
      return null;
    }
  }, [t]);

  useEffect(() => {
    void getWiFi();
  }, [getWiFi]);

  async function connect() {
    setMessage('');

    if (!ssid || !password) return;

    if (readOnly || !isSupported || status !== '') return;
    setStatus('connecting');

    try {
      const rsp = await api.connectWifi(ssid, password);
      if (rsp.code !== 0 || rsp.data?.applied !== true || rsp.data?.rejection != null) {
        setMessage(t('settings.network.wifi.failed'));
        return;
      }

      const observed = await getWiFi();
      if (!observed || !observed.connected || observed.ssid !== ssid) {
        setMessage(t('settings.network.wifi.failed'));
        return;
      }
      setPassword('');
      setIsModalOpen(false);
    } catch {
      setMessage(t('settings.network.wifi.failed'));
    } finally {
      setStatus('');
    }
  }

  async function disconnect(enable: boolean) {
    if (readOnly || enable || !isSupported || status !== '') return;

    setStatus('disconnecting');
    setMessage('');

    try {
      const rsp = await api.disconnectWifi();
      if (rsp.code !== 0 || rsp.data?.applied !== true || rsp.data?.rejection != null) {
        setMessage(t('settings.network.wifi.failed'));
        return;
      }

      const observed = await getWiFi();
      if (!observed || observed.connected) setMessage(t('settings.network.wifi.failed'));
    } catch {
      setMessage(t('settings.network.wifi.failed'));
    } finally {
      setStatus('');
    }
  }

  function openModal() {
    if (readOnly) return;
    setSsid('');
    setPassword('');
    setMessage('');
    setIsModalOpen(true);
  }

  function closeModal() {
    if (status !== '') return;
    setIsModalOpen(false);
    setPassword('');
  }

  if (!isSupported) {
    return message ? (
      <span role="alert" className="text-sm text-red-500">
        {message}
      </span>
    ) : null;
  }

  if (isAPMode) {
    return (
      <div className="flex items-center justify-between">
        <div className="flex flex-col space-y-1">
          <span>{t('settings.network.wifi.title')}</span>
          <span className="text-xs text-neutral-500">{t('settings.network.wifi.apMode')}</span>
        </div>

        <Button shape="round" size="small" disabled>
          <div className="flex items-center justify-center px-1.5">
            <WifiIcon size={16} />
          </div>
        </Button>
      </div>
    );
  }

  return (
    <>
      <div className="flex items-center justify-between">
        <div className="flex flex-col space-y-1">
          <span>{t('settings.network.wifi.title')}</span>
          <span className="text-xs text-neutral-500">
            {connectedWiFi ? connectedWiFi : t('settings.network.wifi.description')}
          </span>
        </div>

        <Button
          aria-label="Configure Wi-Fi"
          disabled={readOnly || status !== ''}
          type={connectedWiFi ? 'primary' : 'default'}
          shape="round"
          size="small"
          onClick={openModal}
        >
          <div className="flex items-center justify-center px-1.5">
            <WifiIcon size={16} />
          </div>
        </Button>
      </div>

      <Modal
        closable={false}
        open={isModalOpen}
        centered={true}
        onOk={connect}
        onCancel={closeModal}
        okText={t('settings.network.wifi.joinBtn')}
        cancelText={t('settings.network.wifi.cancelBtn')}
        confirmLoading={status === 'connecting'}
        okButtonProps={{ disabled: readOnly || status !== '' }}
      >
        {/* title */}
        <div className="flex items-center space-x-5">
          <div className="h-[64px] w-[64px]">
            <WifiPenIcon size={64} className="text-blue-400" />
          </div>

          {!connectedWiFi ? (
            <div className="flex flex-col">
              <span className="text-lg font-bold">{t('settings.network.wifi.connect')}</span>
              <span className="text-xs text-neutral-400">
                {t('settings.network.wifi.connectDesc1')}
              </span>
            </div>
          ) : (
            <div className="flex justify-center">
              <div className="flex w-[300px] justify-between rounded-lg bg-neutral-800">
                <div className="flex w-full justify-between p-3">
                  <span>{connectedWiFi}</span>
                  <Switch
                    aria-label="Wi-Fi connection"
                    disabled={readOnly || status !== ''}
                    value={!!connectedWiFi}
                    loading={status === 'disconnecting'}
                    onChange={disconnect}
                  />
                </div>
              </div>
            </div>
          )}
        </div>

        {/* form */}
        <div className="flex flex-col items-center space-y-3 py-6">
          <Input
            value={ssid}
            style={{ width: '300px' }}
            prefix={<WifiOutlined />}
            placeholder={t('settings.network.wifi.ssid')}
            onChange={(e) => setSsid(e.target.value)}
          />
          <Input.Password
            value={password}
            style={{ width: '300px' }}
            prefix={<LockOutlined />}
            placeholder={t('settings.network.wifi.password')}
            onChange={(e) => setPassword(e.target.value)}
          />

          {!!message && (
            <span role="alert" className="text-sm text-red-500">
              {message}
            </span>
          )}
        </div>
      </Modal>
    </>
  );
};
