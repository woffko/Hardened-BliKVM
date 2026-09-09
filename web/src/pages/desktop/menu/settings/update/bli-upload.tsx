import type { ApplicationUpdateStatus } from '@/types/application-update.ts';
import { Alert, Button, Modal, Progress } from 'antd';
import { useTranslation } from 'react-i18next';

import { validApplicationArchiveSize } from '@/lib/application-update.ts';
import { useApplicationActivation } from '@/hooks/useApplicationActivation.ts';
import { useApplicationUpload } from '@/hooks/useApplicationUpload.ts';

export const BliApplicationUpload = ({
  enabled,
  ready,
  status,
  refresh
}: {
  enabled: boolean;
  ready: boolean;
  status: ApplicationUpdateStatus | null;
  refresh: () => void;
}) => {
  const { t } = useTranslation();
  const upload = useApplicationUpload(enabled);
  const activation = useApplicationActivation(
    enabled && !upload.busy,
    ready,
    status,
    upload.receipt?.slot ?? null,
    refresh
  );
  if (!enabled) return null;
  const text = (key: string) => t(`settings.applicationUpdate.upload.${key}`);
  const activateText = (key: string) => t(`settings.applicationUpdate.activation.${key}`);
  const uploadReady = ready && !!status && !status.pending && !activation.busy;
  return (
    <section className="space-y-3" data-testid="bli-application-upload" aria-label={text('title')}>
      <div className="text-sm">{text('title')}</div>
      <div className="text-sm text-neutral-400">{text('description')}</div>
      <label className="block text-sm">
        <span className="mb-2 block">{text('choose')}</span>
        <input
          type="file"
          disabled={upload.busy || activation.busy || activation.phase === 'confirming'}
          onChange={(event) => upload.select(event.currentTarget.files?.[0] ?? null)}
          className="block w-full text-sm"
          data-testid="application-archive"
        />
      </label>
      <div className="flex flex-wrap gap-2">
        <Button
          type="primary"
          disabled={
            !uploadReady ||
            upload.busy ||
            activation.phase === 'confirming' ||
            !upload.file ||
            !validApplicationArchiveSize(upload.file.size)
          }
          onClick={() => void upload.start()}
        >
          {text(
            upload.state === 'uncertain' ||
              upload.state === 'cancelled' ||
              upload.state === 'busyOrQuota'
              ? 'retry'
              : 'start'
          )}
        </Button>
        {upload.busy && <Button onClick={upload.cancel}>{text('cancel')}</Button>}
      </div>
      {!uploadReady && <Alert type="info" showIcon message={text('notReady')} />}
      {upload.busy && (
        <div role="status" aria-live="polite">
          <Progress percent={upload.percent} status="active" />
          <div className="text-sm text-neutral-400">{text(upload.state)}</div>
        </div>
      )}
      {['invalidFile', 'rejected', 'busyOrQuota', 'uncertain', 'cancelled'].includes(
        upload.state
      ) && <Alert type="warning" showIcon message={text(upload.state)} />}
      {upload.receipt && (
        <Alert
          type="success"
          showIcon
          message={text(upload.receipt.disposition)}
          description={
            <div className="space-y-1 break-all">
              <div>
                {upload.receipt.slot.version} · #{upload.receipt.slot.sequence}
              </div>
              <div className="font-mono text-xs">{upload.receipt.slot.slotId}</div>
              <div>{text('stagedOnly')}</div>
            </div>
          }
        />
      )}
      {upload.receipt && (
        <Button
          data-testid="application-activation-review"
          disabled={!activation.canReview || activation.busy}
          onClick={activation.review}
        >
          {activateText('review')}
        </Button>
      )}
      {!['idle', 'confirming'].includes(activation.phase) && (
        <Alert
          data-testid="application-activation-result"
          showIcon
          type={
            activation.phase === 'installed'
              ? 'success'
              : ['sending', 'awaiting'].includes(activation.phase)
                ? 'info'
                : 'warning'
          }
          message={activateText(activation.phase)}
          description={
            <div className="space-y-2 break-all">
              <div>
                {activation.intent?.target.version} · #{activation.intent?.target.sequence}
              </div>
              {activation.receipt && (
                <div className="font-mono text-xs">{activation.receipt.receipt.operationId}</div>
              )}
              <Button onClick={refresh}>{activateText('reconcile')}</Button>
            </div>
          }
        />
      )}
      <Modal
        open={activation.phase === 'confirming'}
        title={activateText('title')}
        onCancel={activation.close}
        onOk={() => void activation.confirm()}
        okText={activateText('confirm')}
        cancelText={activateText('cancel')}
        okButtonProps={{
          disabled: !activation.canConfirm,
          'data-testid': 'application-activation-confirm'
        }}
      >
        {activation.intent && (
          <div className="space-y-3">
            <p>{activateText('warning')}</p>
            <div>
              {activateText('current')}: {activation.intent.current.version} · #
              {activation.intent.current.sequence}
            </div>
            <div>
              {activateText('target')}: {activation.intent.target.version} · #
              {activation.intent.target.sequence}
            </div>
            <div className="break-all font-mono text-xs">{activation.intent.target.slotId}</div>
            <div>
              {t('settings.applicationUpdate.generation')}: {activation.intent.expectedGeneration}
            </div>
            {!activation.canConfirm && (
              <Alert type="warning" showIcon message={activateText('staleIntent')} />
            )}
          </div>
        )}
      </Modal>
    </section>
  );
};
