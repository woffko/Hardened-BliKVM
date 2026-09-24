import { useCallback, useEffect, useRef, useState } from 'react';
import { Divider } from 'antd';
import { useAtomValue } from 'jotai';
import { LoaderCircleIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/extensions/tailscale.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';

import { Device } from './device.tsx';
import { Header } from './header.tsx';
import { Install } from './install.tsx';
import { Login } from './login.tsx';
import { NativeTailscale } from './native.tsx';
import { Run } from './run.tsx';
import type { Status } from './types.ts';

type TailscaleProps = {
  setIsLocked: (isLocked: boolean) => void;
};

export const Tailscale = (props: TailscaleProps) => {
  const capabilities = useAtomValue(capabilitiesAtom);
  return capabilities?.platform.id === 'bli-h616-pikvm' ? (
    <NativeTailscale />
  ) : (
    <ReferenceTailscale {...props} />
  );
};

const ReferenceTailscale = ({ setIsLocked }: TailscaleProps) => {
  const { t } = useTranslation();

  const [isLoading, setIsLoading] = useState(false);
  const [status, setStatus] = useState<Status>();
  const [errMsg, setErrMsg] = useState('');
  const loadingRef = useRef(false);

  const getStatus = useCallback(() => {
    if (loadingRef.current) return;
    loadingRef.current = true;
    setIsLoading(true);

    api
      .getStatus()
      .then((rsp) => {
        if (rsp.code !== 0) {
          setErrMsg(rsp.msg);
          return;
        }

        setStatus(rsp.data);
      })
      .catch((err) => {
        setErrMsg(err?.message || 'Failed to get status');
      })
      .finally(() => {
        loadingRef.current = false;
        setIsLoading(false);
      });
  }, []);

  useEffect(() => {
    getStatus();
  }, [getStatus]);

  return (
    <>
      <Header state={status?.state} onSuccess={getStatus} />
      <Divider className="opacity-50" />

      {isLoading ? (
        <div className="flex w-full items-center justify-center space-x-2 pt-5 text-neutral-500">
          <LoaderCircleIcon className="animate-spin" size={18} />
          <span>{t('settings.tailscale.loading')}</span>
        </div>
      ) : (
        <>
          {status?.state === 'notInstall' && (
            <Install setIsLocked={setIsLocked} onSuccess={getStatus} />
          )}

          {status?.state === 'notRunning' && <Run onSuccess={getStatus} />}

          {status?.state === 'notLogin' && <Login onSuccess={getStatus} />}

          {(status?.state === 'stopped' || status?.state === 'running') && (
            <Device status={status} onLogout={getStatus} />
          )}

          {errMsg && <div className="pt-5 text-red-500">{errMsg}</div>}
        </>
      )}
    </>
  );
};
