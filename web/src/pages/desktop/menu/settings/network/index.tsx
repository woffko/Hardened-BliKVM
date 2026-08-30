import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { Button, Divider } from 'antd';
import { CheckIcon, SaveIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { SystemActionHandle, SystemActionState } from '../system/action.ts';
import { DNS } from './dns.tsx';
import { IPv6 } from './ipv6.tsx';
import { Tls } from './tls.tsx';
import type { NetworkSectionHandle, NetworkSectionStatus } from './types.ts';
import { idleNetworkSectionStatus } from './types.ts';
import { Wifi } from './wifi.tsx';

type NetworkProps = {
  showTitle?: boolean;
  onActionStateChange?: (state: SystemActionState) => void;
};

export const Network = forwardRef<SystemActionHandle, NetworkProps>(
  ({ showTitle = true, onActionStateChange }, ref) => {
    const { t } = useTranslation();
    const dnsRef = useRef<NetworkSectionHandle>(null);
    const ipv6Ref = useRef<NetworkSectionHandle>(null);
    const [dnsStatus, setDNSStatus] = useState<NetworkSectionStatus>(idleNetworkSectionStatus);
    const [ipv6Status, setIPv6Status] = useState<NetworkSectionStatus>(idleNetworkSectionStatus);
    const [isApplying, setIsApplying] = useState(false);

    const applyNetwork = useCallback(async () => {
      if (isApplying) return;

      setIsApplying(true);
      try {
        const ipv6Result = await ipv6Ref.current?.apply();
        if (ipv6Result?.error) return;

        const dnsResult = await dnsRef.current?.apply();
        if (dnsResult?.error) return;
      } finally {
        setIsApplying(false);
      }
    }, [isApplying]);

    const statuses = [dnsStatus, ipv6Status];
    const hasPending = statuses.some((status) => status.hasPending);
    const hasInvalid = statuses.some((status) => status.hasInvalid);
    const isBusy = isApplying || statuses.some((status) => status.isLoading || status.isSaving);
    const canApply =
      hasPending &&
      !hasInvalid &&
      !isBusy &&
      statuses.every((status) => !status.hasPending || status.canApply);
    const status =
      statuses.find((item) => item.statusKind === 'error' && item.statusText) ||
      statuses.find((item) => item.statusKind === 'success' && item.statusText) ||
      statuses.find((item) => item.statusKind === 'warning' && item.statusText);
    const statusText = status?.statusText || (hasPending ? t('settings.network.dns.unsaved') : '');
    const statusColor =
      status?.statusKind === 'error'
        ? 'text-red-400'
        : status?.statusKind === 'success'
          ? 'text-green-400'
          : 'text-yellow-400/80';

    useImperativeHandle(ref, () => ({ save: applyNetwork }), [applyNetwork]);

    useEffect(() => {
      onActionStateChange?.({
        hasPending,
        hasInvalid,
        canSave: canApply,
        isBusy,
        statusText,
        statusKind: status?.statusKind || (hasPending ? 'warning' : ''),
        label: t('settings.network.dns.apply')
      });
    }, [
      canApply,
      hasInvalid,
      hasPending,
      isBusy,
      onActionStateChange,
      status?.statusKind,
      statusText,
      t
    ]);

    return (
      <>
        {showTitle && (
          <>
            <div className="text-base">{t('settings.network.title')}</div>
            <Divider className="opacity-50" />
          </>
        )}

        <div className="flex flex-col space-y-8">
          <Tls />
          <Wifi />
        </div>

        <Divider className="opacity-50" style={{ margin: '32px 0' }} />

        <div className="flex flex-col space-y-8">
          <DNS
            ref={dnsRef}
            showFooter={false}
            disabled={isApplying}
            onStatusChange={setDNSStatus}
          />
          <IPv6
            ref={ipv6Ref}
            showFooter={false}
            disabled={isApplying}
            onStatusChange={setIPv6Status}
          />

          {!onActionStateChange && (hasPending || statusText) && (
            <div className="flex flex-col items-stretch gap-3 sm:flex-row sm:items-center sm:justify-between">
              <span className={`text-xs ${statusColor}`}>{statusText}</span>
              <Button
                className="w-full sm:w-auto"
                type={hasPending ? 'primary' : 'default'}
                icon={
                  !hasPending && status?.statusKind === 'success' ? (
                    <CheckIcon size={14} />
                  ) : (
                    <SaveIcon size={14} />
                  )
                }
                loading={isApplying}
                disabled={!canApply}
                onClick={() => void applyNetwork()}
              >
                {t('settings.network.dns.apply')}
              </Button>
            </div>
          )}
        </div>
      </>
    );
  }
);

Network.displayName = 'Network';
