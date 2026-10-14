import { useState } from 'react';
import { Button, Checkbox, Input, Modal } from 'antd';
import { useAtom, useAtomValue } from 'jotai';
import { ClipboardCopyIcon, LockIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/target-agent.ts';
import { agentKeyboardAtom, agentLayoutSyncAtom, targetAgentAtom } from '@/jotai/target-agent.ts';

/**
 * What the target agent does from the Keyboard menu, while it is connected
 * (docs/design/target-agent-v1.md): switch the target's layout, keep it in
 * step with the on-screen keyboard's language, the clipboard's text both
 * ways, and lock the session. Each is the operator's action, made under the
 * keyboard's and mouse's lease.
 */
export const AgentKeyboard = () => {
  const { t } = useTranslation();
  const connected = Boolean(useAtomValue(targetAgentAtom)?.agent);
  const [keyboard, setKeyboard] = useAtom(agentKeyboardAtom);
  const [sync, setSync] = useAtom(agentLayoutSyncAtom);
  const [clipboardOpen, setClipboardOpen] = useState(false);
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');

  if (!connected) return null;

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError('');
    setNotice('');
    try {
      await action();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  const switchTo = (id: string) =>
    run(async () => {
      await api.askTargetAgent({ set_layout: { id } });
      const answer = await api.askTargetAgent('keyboard');
      if (typeof answer === 'object' && 'keyboard' in answer) setKeyboard(answer.keyboard);
    });

  const readClipboard = () =>
    run(async () => {
      const answer = await api.askTargetAgent('read_clipboard');
      if (typeof answer === 'object' && 'clipboard' in answer) setText(answer.clipboard);
    });

  const writeClipboard = () =>
    run(async () => {
      if (new TextEncoder().encode(text).length > api.MAX_CLIPBOARD_BYTES)
        throw new Error(t('settings.device.targetAgent.clipboardTooLong'));
      await api.askTargetAgent({ write_clipboard: { text } });
      setNotice(t('settings.device.targetAgent.clipboardSent'));
    });

  const lock = () =>
    Modal.confirm({
      title: t('settings.device.targetAgent.lock'),
      content: t('settings.device.targetAgent.lockConfirm'),
      okText: t('settings.device.targetAgent.lock'),
      cancelText: t('settings.device.cancelBtn'),
      onOk: () => run(() => api.askTargetAgent('lock').then(() => undefined))
    });

  return (
    <div className="flex flex-col space-y-1 px-3 py-1 text-xs" data-testid="agent-keyboard">
      {keyboard && keyboard.installed.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {keyboard.installed.map((layout) => (
            <Button
              key={layout.id}
              size="small"
              type={keyboard.current?.id === layout.id ? 'primary' : 'default'}
              disabled={busy}
              title={layout.name}
              onClick={() => switchTo(layout.id)}
            >
              {layout.language || layout.id}
            </Button>
          ))}
        </div>
      )}
      <Checkbox checked={sync} onChange={(event) => setSync(event.target.checked)}>
        <span className="text-xs">{t('settings.device.targetAgent.layoutSync')}</span>
      </Checkbox>
      <div className="flex gap-1">
        <Button
          size="small"
          icon={<ClipboardCopyIcon size={14} />}
          onClick={() => setClipboardOpen(true)}
        >
          {t('settings.device.targetAgent.clipboard')}
        </Button>
        <Button size="small" icon={<LockIcon size={14} />} disabled={busy} onClick={lock}>
          {t('settings.device.targetAgent.lock')}
        </Button>
      </div>
      {error && (
        <span role="alert" className="text-red-400">
          {error}
        </span>
      )}
      <Modal
        open={clipboardOpen}
        title={t('settings.device.targetAgent.clipboard')}
        footer={null}
        onCancel={() => setClipboardOpen(false)}
      >
        <Input.TextArea
          data-testid="agent-clipboard-text"
          rows={6}
          value={text}
          onChange={(event) => setText(event.target.value)}
        />
        <div className="mt-2 flex gap-2">
          <Button disabled={busy} onClick={readClipboard}>
            {t('settings.device.targetAgent.clipboardRead')}
          </Button>
          <Button type="primary" disabled={busy} onClick={writeClipboard}>
            {t('settings.device.targetAgent.clipboardWrite')}
          </Button>
          <Button
            disabled={!text || !('clipboard' in navigator)}
            onClick={() => void navigator.clipboard.writeText(text).catch(() => undefined)}
          >
            {t('settings.device.targetAgent.clipboardCopyHere')}
          </Button>
        </div>
        {notice && <div className="mt-2 text-xs text-neutral-400">{notice}</div>}
        {error && (
          <div role="alert" className="mt-2 text-xs text-red-400">
            {error}
          </div>
        )}
      </Modal>
    </div>
  );
};
