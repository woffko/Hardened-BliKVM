import type { ReactNode } from 'react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { Alert, Button, Divider, message, Modal, Segmented, Spin, Tag } from 'antd';
import type { TFunction } from 'i18next';
import {
  LockKeyholeIcon,
  RefreshCwIcon,
  ShieldCheckIcon,
  ShieldIcon,
  ShieldOffIcon
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/system-firewall.ts';
import type { FirewallMode, FirewallStatus } from '@/api/system-firewall.ts';

type RulesTab = 'ipv4' | 'ipv6' | 'nft';

type ModeOption = {
  mode: FirewallMode;
  icon: ReactNode;
  title: string;
  description: string;
  tags: string[];
  disabled?: boolean;
  danger?: boolean;
};

export const FirewallSettings = () => {
  const { t } = useTranslation();

  const [status, setStatus] = useState<FirewallStatus | null>(null);
  const [rulesTab, setRulesTab] = useState<RulesTab>('ipv4');
  const [isLoading, setIsLoading] = useState(false);
  const [isApplying, setIsApplying] = useState(false);

  const selectedRules = useMemo(() => {
    if (!status) return '';
    return status.rules[rulesTab] || t('settings.system.firewall.rules.empty');
  }, [status, rulesTab, t]);

  const effectiveMode = status?.effectiveMode || status?.config.mode || 'moderate';
  const isBaselineMode = effectiveMode === 'baseline';
  const isModerateMode = status?.moderateActive || effectiveMode === 'moderate';
  const isRestrictedMode = status?.restrictedActive || effectiveMode === 'restricted';
  const isParanoidMode = status?.paranoidActive || effectiveMode === 'paranoid';

  const modeOptions = useMemo<ModeOption[]>(() => {
    const httpsModeDisabled = !status?.httpsEnabled;
    return [
      {
        mode: 'baseline',
        icon: <ShieldOffIcon size={18} />,
        title: t('settings.system.firewall.mode.baseline'),
        description: t('settings.system.firewall.mode.baselineDesc'),
        tags: [
          t('settings.system.firewall.mode.openTag'),
          t('settings.system.firewall.mode.outboundOpen')
        ],
        danger: true
      },
      {
        mode: 'moderate',
        icon: <ShieldCheckIcon size={18} />,
        title: t('settings.system.firewall.mode.moderate'),
        description: t('settings.system.firewall.mode.moderateDesc'),
        tags: [
          t('settings.system.firewall.mode.default'),
          t('settings.system.firewall.mode.localOnly'),
          t('settings.system.firewall.mode.outboundOpen')
        ]
      },
      {
        mode: 'restricted',
        icon: <ShieldIcon size={18} />,
        title: t('settings.system.firewall.mode.restricted'),
        description: t('settings.system.firewall.mode.restrictedDesc'),
        tags: [
          t('settings.system.firewall.mode.localOnly'),
          t('settings.system.firewall.mode.outboundLimited'),
          ...(httpsModeDisabled ? [t('settings.system.firewall.mode.httpsRequiredTag')] : [])
        ],
        disabled: httpsModeDisabled
      },
      {
        mode: 'paranoid',
        icon: <LockKeyholeIcon size={18} />,
        title: t('settings.system.firewall.mode.paranoid'),
        description: t('settings.system.firewall.mode.paranoidDesc'),
        tags: [
          t('settings.system.firewall.mode.localOnly'),
          t('settings.system.firewall.mode.httpsOnly'),
          ...(httpsModeDisabled ? [t('settings.system.firewall.mode.httpsRequiredTag')] : [])
        ],
        disabled: httpsModeDisabled,
        danger: true
      }
    ];
  }, [status?.httpsEnabled, t]);

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const rsp = await api.getStatus();
      if (rsp.code !== 0 || !rsp.data) {
        message.error(rsp.msg || t('settings.system.firewall.loadFailed'));
        return;
      }
      setStatus(rsp.data);
    } catch {
      message.error(t('settings.system.firewall.loadFailed'));
    } finally {
      setIsLoading(false);
    }
  }, [t]);

  useEffect(() => {
    void load();
  }, [load]);

  function selectMode(mode: FirewallMode) {
    if (!status || isApplying || mode === effectiveMode) return;

    const option = modeOptions.find((item) => item.mode === mode);
    if (!option || option.disabled) {
      if (mode !== 'baseline' && mode !== 'moderate' && !status.httpsEnabled) {
        message.warning(t('settings.system.firewall.enableHttpsFirst'));
      }
      return;
    }

    if (mode === 'restricted') {
      Modal.confirm({
        title: t('settings.system.firewall.restricted.confirmTitle'),
        content: t('settings.system.firewall.restricted.confirmDesc'),
        okText: t('settings.system.firewall.restricted.enable'),
        cancelText: t('settings.system.firewall.cancel'),
        onOk: () => applyMode(mode)
      });
      return;
    }

    if (mode === 'paranoid') {
      Modal.confirm({
        title: t('settings.system.firewall.paranoid.confirmTitle'),
        content: t('settings.system.firewall.paranoid.confirmDesc'),
        okText: t('settings.system.firewall.paranoid.enable'),
        cancelText: t('settings.system.firewall.cancel'),
        onOk: () => applyMode(mode)
      });
      return;
    }

    if (mode === 'baseline') {
      Modal.confirm({
        title: t('settings.system.firewall.baseline.confirmTitle'),
        content: t('settings.system.firewall.baseline.confirmDesc'),
        okText: t('settings.system.firewall.baseline.apply'),
        cancelText: t('settings.system.firewall.cancel'),
        onOk: () => applyMode(mode)
      });
      return;
    }

    Modal.confirm({
      title: modeApplyLabel(t, mode),
      content: option.description,
      okText: modeApplyLabel(t, mode),
      cancelText: t('settings.system.firewall.cancel'),
      onOk: () => applyMode(mode)
    });
  }

  async function applyMode(mode: FirewallMode) {
    setIsApplying(true);
    try {
      const rsp = await api.setMode(mode);
      if (rsp.code !== 0 || !rsp.data) {
        message.error(rsp.msg || t('settings.system.firewall.saveFailed'));
        return;
      }

      let nextStatus: FirewallStatus = rsp.data;
      if (nextStatus.confirmationRequired) {
        const confirmRsp = await api.confirmPending();
        if (confirmRsp.code === 0 && confirmRsp.data) {
          nextStatus = confirmRsp.data;
        }
      }

      setStatus(nextStatus);
      message.success(t('settings.system.firewall.saved'));
    } catch (err) {
      console.log(err);
      message.error(t('settings.system.firewall.saveFailed'));
    } finally {
      setIsApplying(false);
      load();
    }
  }

  if (isLoading && !status) {
    return (
      <div className="flex justify-center py-12">
        <Spin />
      </div>
    );
  }

  return (
    <div className="flex min-w-0 flex-col space-y-6">
      {!status?.httpsEnabled && (
        <Alert type="warning" showIcon message={t('settings.system.firewall.httpsRequired')} />
      )}

      {status && isBaselineMode && (
        <Alert
          type="warning"
          showIcon
          message={t('settings.system.firewall.baseline.active')}
          description={t('settings.system.firewall.baseline.allows')}
        />
      )}

      {status && isModerateMode && (
        <Alert
          type="success"
          showIcon
          message={t('settings.system.firewall.moderate.active')}
          description={t('settings.system.firewall.moderate.allows')}
        />
      )}

      {status && isRestrictedMode && !isParanoidMode && (
        <Alert
          type="warning"
          showIcon
          message={t('settings.system.firewall.restricted.active')}
          description={t('settings.system.firewall.restricted.allows')}
        />
      )}

      {status && isParanoidMode && (
        <Alert
          type="error"
          showIcon
          message={t('settings.system.firewall.paranoid.active')}
          description={t('settings.system.firewall.paranoid.blocks')}
        />
      )}

      <div className="min-w-0 space-y-4 overflow-hidden rounded-md bg-neutral-800/50 p-4">
        <div className="flex min-w-0 flex-col gap-3 sm:flex-row sm:items-start sm:justify-between sm:gap-4">
          <div className="min-w-0 space-y-1">
            <div className="font-semibold text-neutral-100">
              {t('settings.system.firewall.mode.title')}
            </div>
            <div className="break-words text-xs leading-snug text-neutral-500">
              {t('settings.system.firewall.mode.description')}
            </div>
          </div>
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <Tag className="m-0 max-w-full whitespace-normal" color={modeColor(effectiveMode)}>
              {modeText(t, effectiveMode)}
            </Tag>
            <Button
              size="small"
              icon={<RefreshCwIcon size={14} />}
              loading={isLoading}
              onClick={load}
            >
              {t('settings.system.firewall.refresh')}
            </Button>
          </div>
        </div>

        <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
          {modeOptions.map((option) => (
            <ModeChoice
              key={option.mode}
              option={option}
              active={option.mode === effectiveMode}
              current={option.mode === effectiveMode}
              busy={isApplying || !status}
              currentLabel={t('settings.system.firewall.mode.current')}
              onSelect={selectMode}
            />
          ))}
        </div>

        <div className="grid grid-cols-1 gap-3 pt-1 text-xs text-neutral-400 sm:grid-cols-2">
          <StatusLine
            label={t('settings.system.firewall.backend')}
            value={status?.backend.preferred || '-'}
          />
          <StatusLine
            label="HTTPS"
            value={
              status?.httpsEnabled
                ? t('settings.system.firewall.enabledPort', { port: status.httpsPort })
                : t('settings.system.firewall.disabled')
            }
          />
          <StatusLine label="iptables" value={toolText(status?.backend.iptables)} />
          <StatusLine label="ip6tables" value={toolText(status?.backend.ip6tables)} />
          <StatusLine label="nft" value={toolText(status?.backend.nft)} />
          <StatusLine
            label={t('settings.system.firewall.effectiveMode')}
            value={modeText(t, effectiveMode)}
          />
        </div>
      </div>

      <Divider className="opacity-50" />

      <div className="space-y-3">
        <div className="flex min-w-0 flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-2 text-base">
            <ShieldIcon size={16} />
            <span>{t('settings.system.firewall.rules.title')}</span>
          </div>
          <Segmented
            size="small"
            className="max-w-full overflow-x-auto"
            value={rulesTab}
            options={[
              { value: 'ipv4', label: 'IPv4' },
              { value: 'ipv6', label: 'IPv6' },
              { value: 'nft', label: 'nft' }
            ]}
            onChange={(value) => setRulesTab(value as RulesTab)}
          />
        </div>
        <pre className="max-h-[360px] min-h-[260px] overflow-auto rounded bg-black/50 p-3 font-mono text-xs leading-relaxed text-neutral-200">
          {selectedRules}
        </pre>
      </div>
    </div>
  );
};

function ModeChoice({
  option,
  active,
  current,
  busy,
  currentLabel,
  onSelect
}: {
  option: ModeOption;
  active: boolean;
  current: boolean;
  busy: boolean;
  currentLabel: string;
  onSelect: (mode: FirewallMode) => void;
}) {
  const disabled = busy || Boolean(option.disabled);

  return (
    <button
      type="button"
      className={modeButtonClass(active, disabled, option.danger)}
      disabled={disabled}
      onClick={() => onSelect(option.mode)}
    >
      <span className={modeIconClass(active, option.danger)}>{option.icon}</span>
      <span className="min-w-0 flex-1 space-y-2 overflow-hidden text-left">
        <span className="flex min-w-0 items-center justify-between gap-2">
          <span className="truncate text-sm font-medium text-neutral-100">{option.title}</span>
          {current && (
            <Tag className="m-0 max-w-[55%] shrink-0 truncate" color={modeColor(option.mode)}>
              {currentLabel}
            </Tag>
          )}
        </span>
        <span className="block break-words text-xs leading-snug text-neutral-400">
          {option.description}
        </span>
        <span className="flex min-w-0 flex-wrap gap-1">
          {option.tags.map((tag) => (
            <Tag
              key={tag}
              className="m-0 max-w-full whitespace-normal break-words leading-snug"
              color={tagColor(option.mode, option.danger)}
            >
              {tag}
            </Tag>
          ))}
        </span>
      </span>
    </button>
  );
}

function modeButtonClass(active: boolean, disabled: boolean, danger?: boolean) {
  const base =
    'flex w-full min-w-0 items-start gap-3 overflow-hidden rounded-md border p-3 text-left transition-colors';
  if (disabled && !active) {
    return `${base} cursor-not-allowed border-neutral-800 bg-neutral-900/35 opacity-60`;
  }
  if (active) {
    return danger
      ? `${base} cursor-default border-red-500/60 bg-red-500/10`
      : `${base} cursor-default border-blue-500/60 bg-blue-500/10`;
  }
  if (danger) {
    return `${base} border-neutral-700 bg-neutral-900/30 hover:border-red-500/60 hover:bg-red-500/10`;
  }
  return `${base} border-neutral-700 bg-neutral-900/30 hover:border-blue-500/60 hover:bg-blue-500/10`;
}

function modeIconClass(active: boolean, danger?: boolean) {
  const base = 'flex h-9 w-9 shrink-0 items-center justify-center rounded-md';
  if (danger) {
    return active ? `${base} bg-red-500/20 text-red-200` : `${base} bg-neutral-800 text-red-300`;
  }
  return active ? `${base} bg-blue-500/20 text-blue-200` : `${base} bg-neutral-800 text-blue-300`;
}

function tagColor(mode: FirewallMode, danger?: boolean) {
  if (mode === 'moderate') return 'green';
  if (mode === 'restricted') return 'orange';
  if (danger) return 'red';
  return 'blue';
}

function StatusLine({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-w-0 justify-between gap-3">
      <span className="min-w-0 break-words">{label}</span>
      <span className="truncate text-right text-neutral-200">{value}</span>
    </div>
  );
}

function toolText(tool?: { installed: boolean; detail: string }) {
  if (!tool) return '-';
  return tool.installed ? tool.detail : 'missing';
}

function modeText(t: TFunction, mode: FirewallMode) {
  switch (mode) {
    case 'baseline':
      return t('settings.system.firewall.mode.baseline');
    case 'moderate':
      return t('settings.system.firewall.mode.moderate');
    case 'restricted':
      return t('settings.system.firewall.mode.restricted');
    case 'paranoid':
      return t('settings.system.firewall.mode.paranoid');
  }
}

function modeApplyLabel(t: TFunction, mode: FirewallMode) {
  switch (mode) {
    case 'baseline':
      return t('settings.system.firewall.baseline.apply');
    case 'moderate':
      return t('settings.system.firewall.moderate.apply');
    case 'restricted':
      return t('settings.system.firewall.restricted.enable');
    case 'paranoid':
      return t('settings.system.firewall.paranoid.enable');
  }
}

function modeColor(mode: FirewallMode) {
  switch (mode) {
    case 'baseline':
      return 'red';
    case 'moderate':
      return 'green';
    case 'restricted':
      return 'orange';
    case 'paranoid':
      return 'red';
  }
}
