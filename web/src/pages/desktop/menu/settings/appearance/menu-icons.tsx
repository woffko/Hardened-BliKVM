import { useCallback, useEffect, useState } from 'react';
import { Modal, Switch } from 'antd';
import { useAtom, useAtomValue } from 'jotai';
import {
  DiscIcon,
  DownloadIcon,
  FileJsonIcon,
  MaximizeIcon,
  NetworkIcon,
  PowerIcon,
  TerminalSquareIcon,
  XIcon
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as adminApi from '@/api/admin.ts';
import * as downloadApi from '@/api/download.ts';
import * as vmApi from '@/api/vm.ts';
import { isAvailable, supportsMenuItem, type CapabilityMenuItem } from '@/lib/capabilities.ts';
import * as ls from '@/lib/localstorage.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { menuDisabledItemsAtom } from '@/jotai/settings.ts';
import { Robot } from '@/components/icons/robot.tsx';

export const MenuIcons = () => {
  const { t } = useTranslation();

  const capabilities = useAtomValue(capabilitiesAtom);
  const [menuDisabledItems, setMenuDisabledItems] = useAtom(menuDisabledItemsAtom);
  const [isTerminalWarningOpen, setIsTerminalWarningOpen] = useState(false);
  const [isRemoteDownloadWarningOpen, setIsRemoteDownloadWarningOpen] = useState(false);
  const [isTerminalLoading, setIsTerminalLoading] = useState(false);
  const [isRemoteDownloadEnabled, setIsRemoteDownloadEnabled] = useState(false);
  const [isRemoteDownloadLoading, setIsRemoteDownloadLoading] = useState(false);

  const terminalSupported = supportsMenuItem(capabilities, 'terminal');
  const remoteDownloadSupported = Boolean(
    capabilities && isAvailable(capabilities.virtualMedia.remoteDownload)
  );

  const items = [
    { key: 'image', icon: <DiscIcon size={16} /> },
    { key: 'download', icon: <DownloadIcon size={16} /> },
    {
      key: 'remoteDownload',
      icon: <DownloadIcon size={16} />,
      label: 'settings.appearance.menuBar.remoteDownload'
    },
    { key: 'terminal', icon: <TerminalSquareIcon size={16} /> },
    { key: 'script', icon: <FileJsonIcon size={16} /> },
    { key: 'wol', icon: <NetworkIcon size={16} /> },
    { key: 'picoclaw', icon: <Robot size={16} /> },
    { key: 'power', icon: <PowerIcon size={16} /> },
    { key: 'fullscreen', icon: <MaximizeIcon size={16} />, label: 'fullscreen.toggle' },
    { key: 'collapse', icon: <XIcon size={16} />, label: 'menu.collapse' }
  ].filter((item) =>
    item.key === 'remoteDownload'
      ? remoteDownloadSupported
      : supportsMenuItem(capabilities, item.key as CapabilityMenuItem)
  );

  const setItemVisible = useCallback(
    (key: string, visible: boolean) => {
      setMenuDisabledItems((currentItems) => {
        const newItems = visible
          ? currentItems.filter((item) => item !== key)
          : Array.from(new Set([...currentItems, key]));
        ls.setMenuDisabledItems(newItems);
        return newItems;
      });
    },
    [setMenuDisabledItems]
  );

  // Scripts and PicoClaw are switched on the server as well: the owner persists
  // the switch and the server refuses the feature while it is off.
  const [serverSwitchLoading, setServerSwitchLoading] = useState('');
  const scriptsSupported = supportsMenuItem(capabilities, 'script');
  const picoclawSupported = supportsMenuItem(capabilities, 'picoclaw');

  useEffect(() => {
    if (!scriptsSupported && !picoclawSupported) return;
    adminApi.getAdminFeatures().then((rsp) => {
      if (rsp.code !== 0 || !rsp.data) return;
      if (scriptsSupported && typeof rsp.data.scripts === 'boolean') {
        setItemVisible('script', rsp.data.scripts);
      }
      if (picoclawSupported && typeof rsp.data.picoclaw === 'boolean') {
        setItemVisible('picoclaw', rsp.data.picoclaw);
      }
    });
  }, [picoclawSupported, scriptsSupported, setItemVisible]);

  function updateServerSwitch(key: string, feature: adminApi.AdminFeature, enabled: boolean) {
    if (serverSwitchLoading) return;
    setServerSwitchLoading(key);
    adminApi
      .setAdminFeature(feature, enabled)
      .then((rsp) => {
        if (rsp.code !== 0 || typeof rsp.data?.enabled !== 'boolean') return;
        setItemVisible(key, rsp.data.enabled);
      })
      .finally(() => setServerSwitchLoading(''));
  }

  useEffect(() => {
    if (terminalSupported) {
      vmApi.getTerminalEnabled().then((rsp) => {
        if (rsp.code !== 0 || typeof rsp.data?.enabled !== 'boolean') return;
        setItemVisible('terminal', rsp.data.enabled);
      });
    }
    if (remoteDownloadSupported) {
      downloadApi.getRemoteImageDownloadEnabled().then((rsp) => {
        if (rsp.code !== 0 || typeof rsp.data?.remoteEnabled !== 'boolean') return;
        setIsRemoteDownloadEnabled(rsp.data.remoteEnabled);
      });
    }
  }, [remoteDownloadSupported, setItemVisible, terminalSupported]);

  function updateItems(key: string) {
    const enabled = !menuDisabledItems.includes(key);
    const nextEnabled = !enabled;

    if (key === 'terminal') {
      if (nextEnabled) {
        setIsTerminalWarningOpen(true);
      } else {
        updateTerminal(false);
      }
      return;
    }

    if (key === 'script' && scriptsSupported) {
      updateServerSwitch(key, 'scripts', nextEnabled);
      return;
    }

    if (key === 'picoclaw' && picoclawSupported) {
      updateServerSwitch(key, 'picoclaw', nextEnabled);
      return;
    }

    if (key === 'remoteDownload') {
      if (!isRemoteDownloadEnabled) {
        setIsRemoteDownloadWarningOpen(true);
      } else {
        updateRemoteDownload(false);
      }
      return;
    }

    setItemVisible(key, nextEnabled);
  }

  function updateTerminal(enabled: boolean) {
    if (isTerminalLoading) return;
    setIsTerminalLoading(true);

    vmApi
      .setTerminalEnabled(enabled)
      .then((rsp) => {
        if (rsp.code !== 0) return;
        setItemVisible('terminal', enabled);
        setIsTerminalWarningOpen(false);
      })
      .finally(() => setIsTerminalLoading(false));
  }

  function updateRemoteDownload(enabled: boolean) {
    if (isRemoteDownloadLoading) return;
    setIsRemoteDownloadLoading(true);

    downloadApi
      .setRemoteImageDownloadEnabled(enabled)
      .then((rsp) => {
        if (rsp.code !== 0) return;
        setIsRemoteDownloadEnabled(enabled);
        setIsRemoteDownloadWarningOpen(false);
      })
      .finally(() => setIsRemoteDownloadLoading(false));
  }

  function itemChecked(key: string) {
    if (key === 'remoteDownload') {
      return isRemoteDownloadEnabled;
    }
    return !menuDisabledItems.includes(key);
  }

  function itemLoading(key: string) {
    if (serverSwitchLoading === key) {
      return true;
    }
    if (key === 'terminal') {
      return isTerminalLoading;
    }
    if (key === 'remoteDownload') {
      return isRemoteDownloadLoading;
    }
    return false;
  }

  return (
    <>
      <div className="mt-8 flex min-w-0 flex-col space-y-5">
        <div className="flex min-w-0 flex-col">
          <span className="text-neutral-400">{t('settings.appearance.menuBar.icons')}</span>
          <span className="text-xs text-neutral-500">
            {t('settings.appearance.menuBar.iconsDesc')}
          </span>
        </div>

        <div className="mt-5 flex flex-col space-y-5">
          {items.map((item) => (
            <div key={item.key} className="flex min-w-0 items-center justify-between gap-4">
              <div className="flex min-w-0 items-center space-x-2 text-neutral-400">
                <div className="shrink-0">{item.icon}</div>
                <span className="min-w-0 break-words text-neutral-300">
                  {item.label ? t(item.label) : t(`${item.key}.title`)}
                </span>
              </div>

              <Switch
                className="shrink-0"
                checked={itemChecked(item.key)}
                loading={itemLoading(item.key)}
                onChange={() => updateItems(item.key)}
              />
            </div>
          ))}
        </div>
      </div>

      <Modal
        title={t('settings.appearance.menuBar.terminalWarningTitle')}
        open={isTerminalWarningOpen}
        centered={true}
        okType="danger"
        okText={t('settings.appearance.menuBar.terminalWarningConfirm')}
        cancelText={t('settings.appearance.menuBar.terminalWarningCancel')}
        onOk={() => updateTerminal(true)}
        onCancel={() => setIsTerminalWarningOpen(false)}
        confirmLoading={isTerminalLoading}
      >
        <div className="py-4 text-neutral-300">
          {t('settings.appearance.menuBar.terminalWarningDesc')}
        </div>
      </Modal>

      <Modal
        title={t('settings.appearance.menuBar.remoteDownloadWarningTitle')}
        open={isRemoteDownloadWarningOpen}
        centered={true}
        okType="danger"
        okText={t('settings.appearance.menuBar.remoteDownloadWarningConfirm')}
        cancelText={t('settings.appearance.menuBar.remoteDownloadWarningCancel')}
        onOk={() => updateRemoteDownload(true)}
        onCancel={() => setIsRemoteDownloadWarningOpen(false)}
        confirmLoading={isRemoteDownloadLoading}
      >
        <div className="py-4 text-neutral-300">
          {t('settings.appearance.menuBar.remoteDownloadWarningDesc')}
        </div>
      </Modal>
    </>
  );
};
