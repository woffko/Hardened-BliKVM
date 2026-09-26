import { useEffect, useRef, useState } from 'react';
import { Alert, Button, Divider, Modal, Progress, Switch } from 'antd';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/system-update.ts';
import type { RawSystemUpdateState } from '@/api/system-update.ts';
import type { ApiResponse } from '@/lib/http.ts';

const MAX_BUNDLE_BYTES = 8 * 1024 * 1024 * 1024;

class Refused extends Error {}

function reason(error: unknown): string {
  if (error instanceof Refused) return error.message;
  const response = (error as { response?: { data?: { msg?: unknown } } })?.response;
  return typeof response?.data?.msg === 'string' ? response.data.msg : '';
}

/** The server answers refusals with a negative code and a message. */
async function accepted<T>(call: Promise<ApiResponse<T>>): Promise<T> {
  const response = await call;
  if (response.code !== 0) throw new Refused(response.msg);
  return response.data;
}

/**
 * RAW system update (docs/design/system-raw-update-v1.md): off by default,
 * a signed bundle is uploaded and verified, installed by rebooting, and kept
 * or rolled back once the new system reports itself.
 */
export const BliSystemUpdate = () => {
  const { t } = useTranslation();
  const text = (key: string, options?: Record<string, unknown>) =>
    t(`settings.update.system.${key}`, options);
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [switching, setSwitching] = useState(false);
  const [update, setUpdate] = useState<RawSystemUpdateState | null>(null);
  const [file, setFile] = useState<File | null>(null);
  const [percent, setPercent] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const upload = useRef<AbortController | null>(null);

  const refresh = async () => {
    try {
      const switched = await accepted(api.getRawSystemUpdatesEnabled());
      setEnabled(!!switched?.enabled);
      setUpdate((await accepted(api.getRawSystemUpdateStatus())) ?? null);
    } catch (err) {
      setError(reason(err) || text('queryFailed'));
    }
  };

  useEffect(() => {
    void refresh();
    return () => upload.current?.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const changeEnabled = (next: boolean) => {
    const apply = async () => {
      setSwitching(true);
      setError('');
      try {
        const switched = await accepted(api.setRawSystemUpdatesEnabled(next));
        setEnabled(!!switched?.enabled);
      } catch (err) {
        setError(reason(err) || text('installFailed'));
      } finally {
        setSwitching(false);
        void refresh();
      }
    };
    if (!next) {
      void apply();
      return;
    }
    Modal.confirm({
      title: text('rawEnableConfirmTitle'),
      content: (
        <div className="space-y-3">
          <div>{text('rawEnableConfirmDesc')}</div>
          <Alert type="warning" showIcon message={text('rawRollbackNote')} />
        </div>
      ),
      okText: t('settings.update.confirm'),
      cancelText: t('settings.update.cancel'),
      onOk: apply
    });
  };

  const startUpload = async () => {
    if (!file || upload.current) return;
    const controller = new AbortController();
    upload.current = controller;
    setError('');
    setNotice('');
    setPercent(0);
    try {
      const staged = await accepted(
        api.uploadSystemBundle(file, controller.signal, (loaded) =>
          setPercent(Math.min(100, Math.floor((loaded / file.size) * 100)))
        )
      );
      setUpdate(staged ?? null);
      setNotice(text('stagedDesc'));
    } catch (err) {
      setError(reason(err) || text('downloadFailed'));
    } finally {
      upload.current = null;
      setPercent(null);
      void refresh();
    }
  };

  const run = async (
    action: () => Promise<ApiResponse<unknown>>,
    done: string,
    reboots = false
  ) => {
    setBusy(true);
    setError('');
    setNotice('');
    try {
      const result = (await accepted(action())) as api.SystemActionOutcome | null;
      if (reboots && result?.applied !== true) {
        throw new Refused(result?.rejection ?? text('installFailed'));
      }
      setNotice(done);
    } catch (err) {
      setError(reason(err) || text('installFailed'));
    } finally {
      setBusy(false);
      void refresh();
    }
  };

  const confirmThen = (title: string, content: string, action: () => void) =>
    Modal.confirm({
      title,
      content,
      okText: t('settings.update.confirm'),
      cancelText: t('settings.update.cancel'),
      onOk: action
    });

  const phase = update?.phase ?? 'none';
  const uploading = percent !== null;
  return (
    <section className="space-y-4" data-testid="bli-system-update" aria-label={text('title')}>
      <Divider className="opacity-50" />
      <div className="flex items-start justify-between gap-3">
        <div>
          <div className="text-base">{text('title')}</div>
          <div className="mt-1 text-sm text-neutral-500">{text('rawEnableDesc')}</div>
        </div>
        <Button size="small" onClick={() => void refresh()}>
          {text('refresh')}
        </Button>
      </div>
      <div className="flex items-center justify-between gap-3">
        <span className="text-sm">{text('rawEnable')}</span>
        <Switch
          checked={!!enabled}
          loading={switching || enabled === null}
          onChange={changeEnabled}
          aria-label={text('rawEnable')}
          data-testid="raw-system-update-switch"
        />
      </div>
      {update && phase !== 'none' && (
        <Alert
          showIcon
          type={['rolled_back', 'failed'].includes(phase) ? 'warning' : 'info'}
          message={`${text(`rawPhase.${phase}`)} · ${update.version} · #${update.sequence}`}
          description={update.message}
          data-testid="raw-system-update-phase"
        />
      )}
      {enabled && (
        <>
          {!['staged', 'writing', 'written', 'booted', 'rollback_requested'].includes(phase) && (
            <div className="space-y-3">
              <div className="text-sm text-neutral-400">{text('rawUploadDesc')}</div>
              <input
                type="file"
                accept=".tar"
                disabled={uploading}
                onChange={(event) => setFile(event.currentTarget.files?.[0] ?? null)}
                className="block w-full text-sm"
                data-testid="system-bundle"
                aria-label={text('rawUpload')}
              />
              <Button
                type="primary"
                disabled={!file || uploading || file.size > MAX_BUNDLE_BYTES}
                onClick={() => void startUpload()}
              >
                {text('rawUploadStart')}
              </Button>
              {uploading && (
                <div role="status" aria-live="polite">
                  <Progress percent={percent ?? 0} status="active" />
                  <div className="text-sm text-neutral-400">
                    {text(percent === 100 ? 'rawVerifying' : 'rawUploading')}
                  </div>
                </div>
              )}
            </div>
          )}
          {phase === 'staged' && (
            <div className="flex flex-wrap gap-2">
              <Button
                type="primary"
                danger
                loading={busy}
                onClick={() =>
                  confirmThen(
                    text('installConfirmTitle'),
                    text('rawInstallDesc'),
                    () => void run(api.installSystemUpdate, text('rawRebooting'), true)
                  )
                }
              >
                {text('install')}
              </Button>
              <Button
                loading={busy}
                onClick={() => void run(api.cancelSystemUpdate, text('rawDiscarded'))}
              >
                {text('rawCancel')}
              </Button>
            </div>
          )}
          {phase === 'booted' && (
            <div className="flex flex-wrap gap-2">
              <Button
                type="primary"
                loading={busy}
                onClick={() => void run(api.confirmSystemUpdate, text('bootGood'))}
              >
                {text('confirmBoot')}
              </Button>
              <Button
                danger
                loading={busy}
                onClick={() =>
                  confirmThen(
                    text('rollbackConfirmTitle'),
                    text('rawRollbackDesc'),
                    () => void run(api.rollbackSystemUpdate, text('rawRebooting'), true)
                  )
                }
              >
                {text('rollback')}
              </Button>
            </div>
          )}
        </>
      )}
      {notice && <Alert type="success" showIcon message={notice} />}
      {error && <Alert type="error" showIcon message={error} />}
    </section>
  );
};
