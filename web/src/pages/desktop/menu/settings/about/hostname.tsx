import { useEffect, useState } from 'react';
import { CheckOutlined, CloseOutlined } from '@ant-design/icons';
import { Button, Input } from 'antd';
import { ClipboardPenIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/vm.ts';

/**
 * The reference's editable hostname. The board owner applies the new name
 * through systemd-hostnamed, so it takes effect at once.
 */
export const Hostname = ({ editable }: { editable: boolean }) => {
  const { t } = useTranslation();

  const [isLoading, setIsLoading] = useState(false);
  const [hostname, setHostname] = useState('');
  const [editState, setEditState] = useState<'' | 'editing' | 'edited'>('');
  const [input, setInput] = useState('');
  const [error, setError] = useState('');

  useEffect(() => {
    api
      .getHostname()
      .then((rsp) => {
        if (rsp.data?.hostname) setHostname(rsp.data.hostname);
      })
      .catch(() => undefined);
  }, []);

  function update() {
    const next = input.trim();
    if (next === hostname) {
      setEditState('');
      return;
    }
    if (isLoading) return;
    setIsLoading(true);
    setError('');
    api
      .setHostname(next)
      .then((rsp) => {
        if (rsp.code !== 0) {
          setError(rsp.msg || 'Failed');
          return;
        }
        setHostname(rsp.data?.hostname || next);
        setEditState('edited');
      })
      .catch(() => setError('Failed'))
      .finally(() => setIsLoading(false));
  }

  return (
    <div className="space-y-1">
      <div className="flex w-full min-w-0 flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <span className="shrink-0">{t('settings.about.hostname')}</span>
        {editState === 'editing' ? (
          <div className="flex items-center space-x-1">
            <Input
              disabled={isLoading}
              style={{ width: 150 }}
              value={input}
              maxLength={63}
              onChange={(e) => setInput(e.target.value)}
              onPressEnter={update}
            />
            <Button size="small" icon={<CheckOutlined />} loading={isLoading} onClick={update} />
            <Button size="small" icon={<CloseOutlined />} onClick={() => setEditState('')} />
          </div>
        ) : (
          <div className="flex items-center space-x-2">
            <span>{hostname || '-'}</span>
            {editable && (
              <div
                role="button"
                aria-label={t('settings.about.hostname')}
                className="size-[16px] cursor-pointer text-neutral-500 hover:text-blue-500"
                onClick={() => {
                  setInput(hostname);
                  setError('');
                  setEditState('editing');
                }}
              >
                <ClipboardPenIcon size={16} />
              </div>
            )}
          </div>
        )}
      </div>
      {editState === 'edited' && (
        <div className="flex w-full justify-end text-xs text-green-500">
          {t('settings.about.hostnameUpdated')}
        </div>
      )}
      {error && <div className="flex w-full justify-end text-xs text-red-400">{error}</div>}
    </div>
  );
};
