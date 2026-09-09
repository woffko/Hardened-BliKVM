import { Alert, Button, Progress } from 'antd';
import { useTranslation } from 'react-i18next';

import { validApplicationArchiveSize } from '@/lib/application-update.ts';
import { useApplicationUpload } from '@/hooks/useApplicationUpload.ts';

export const BliApplicationUpload = ({ enabled, ready }: { enabled: boolean; ready: boolean }) => {
  const { t } = useTranslation();
  const upload = useApplicationUpload(enabled);
  if (!enabled) return null;
  const text = (key: string) => t(`settings.applicationUpdate.upload.${key}`);
  return (
    <section className="space-y-3" data-testid="bli-application-upload" aria-label={text('title')}>
      <div className="text-sm">{text('title')}</div>
      <div className="text-sm text-neutral-400">{text('description')}</div>
      <label className="block text-sm">
        <span className="mb-2 block">{text('choose')}</span>
        <input
          type="file"
          disabled={upload.busy}
          onChange={(event) => upload.select(event.currentTarget.files?.[0] ?? null)}
          className="block w-full text-sm"
          data-testid="application-archive"
        />
      </label>
      <div className="flex flex-wrap gap-2">
        <Button
          type="primary"
          disabled={
            !ready || upload.busy || !upload.file || !validApplicationArchiveSize(upload.file.size)
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
      {!ready && <Alert type="info" showIcon message={text('notReady')} />}
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
    </section>
  );
};
