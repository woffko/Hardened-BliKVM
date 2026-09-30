import type { ApplicationSlot } from '@/types/application-update.ts';
import { Alert, Button, Divider, Spin } from 'antd';
import { useAtomValue } from 'jotai';
import { RefreshCwIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { isWritable } from '@/lib/capabilities.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { useApplicationUpdateStatus } from '@/hooks/useApplicationUpdateStatus.ts';

import { BliSystemUpdate } from './bli-system.tsx';
import { BliApplicationUpload } from './bli-upload.tsx';

const Slot = ({ label, slot }: { label: string; slot: ApplicationSlot }) => (
  <div className="min-w-0 rounded-lg border border-neutral-700/70 bg-neutral-800/40 p-3">
    <div className="text-xs text-neutral-400">{label}</div>
    <div className="mt-2 break-words text-sm text-neutral-100">
      {slot.version} · #{slot.sequence}
    </div>
    <div className="mt-1 break-all font-mono text-xs text-neutral-500">{slot.slotId}</div>
  </div>
);

export const BliApplicationUpdate = () => {
  const { t } = useTranslation();
  const { data, state, refreshing, refresh } = useApplicationUpdateStatus();
  const capabilities = useAtomValue(capabilitiesAtom);
  const writable = !!capabilities && isWritable(capabilities.system.signedApplicationUpdates);
  return (
    <div className="space-y-4" data-testid="bli-application-update">
      <div className="flex items-start justify-between gap-3">
        <div>
          <div className="text-base">{t('settings.applicationUpdate.title')}</div>
          <div className="mt-1 text-sm text-neutral-500">
            {t('settings.applicationUpdate.description')}
          </div>
        </div>
        <Button
          size="small"
          icon={<RefreshCwIcon size={14} />}
          loading={refreshing}
          disabled={refreshing}
          onClick={refresh}
        >
          {t('settings.applicationUpdate.refresh')}
        </Button>
      </div>
      <Divider className="opacity-50" />
      {state === 'loading' && <Spin size="small" />}
      {(state === 'error' || state === 'stale') && (
        <Alert
          showIcon
          type="warning"
          message={t(`settings.applicationUpdate.${state === 'stale' ? 'stale' : 'unavailable'}`)}
        />
      )}
      {data && (
        <>
          <div className="grid gap-3 sm:grid-cols-2">
            <Slot label={t('settings.applicationUpdate.confirmed')} slot={data.confirmed} />
            <Slot label={t('settings.applicationUpdate.selected')} slot={data.selected} />
          </div>
          <div className="flex flex-wrap gap-x-6 gap-y-2 text-sm text-neutral-400">
            <span>
              {t('settings.applicationUpdate.floor')}: {data.floorSequence}
            </span>
            <span>
              {t('settings.applicationUpdate.generation')}: {data.generation}
            </span>
          </div>
          {data.pending ? (
            <Alert
              showIcon
              type="info"
              message={t(`settings.applicationUpdate.phase.${data.pending.phase}`)}
              description={`${data.pending.target.version} · #${data.pending.target.sequence}`}
            />
          ) : state === 'ready' ? (
            <div className="text-sm text-emerald-400">{t('settings.applicationUpdate.idle')}</div>
          ) : null}
          {data.lastRollback && (
            <Alert
              showIcon
              type="warning"
              message={t('settings.applicationUpdate.lastRollback')}
              description={t(`settings.applicationUpdate.rollback.${data.lastRollback}`)}
            />
          )}
        </>
      )}
      <BliApplicationUpload
        enabled={writable}
        ready={state === 'ready' && !refreshing}
        status={data}
        refresh={refresh}
      />
      {!writable && (
        <Alert showIcon type="info" message={t('settings.applicationUpdate.readOnly')} />
      )}
      {writable && <BliSystemUpdate />}
    </div>
  );
};
