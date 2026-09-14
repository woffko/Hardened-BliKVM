import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Button, Segmented } from 'antd';
import { useAtomValue } from 'jotai';
import { SaveIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { supportsSystemSection, systemSectionWritable } from '@/lib/capabilities.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';

import { Network } from '../network';
import { SystemLog } from '../system-log';
import type { SystemActionHandle, SystemActionState } from './action';
import { idleSystemActionState } from './action';
import { FirewallSettings } from './firewall';
import { TimeSettings } from './time';

type SystemSection = 'network' | 'time' | 'firewall' | 'systemLog';

export const System = () => {
  const { t } = useTranslation();
  const capabilities = useAtomValue(capabilitiesAtom);
  const [section, setSection] = useState<SystemSection>('time');
  const [actionState, setActionState] = useState<SystemActionState>(idleSystemActionState);
  const networkRef = useRef<SystemActionHandle>(null);
  const timeRef = useRef<SystemActionHandle>(null);
  const systemLogRef = useRef<SystemActionHandle>(null);

  const allSections = useMemo(
    () => [
      { value: 'network' as const, label: t('settings.network.title') },
      { value: 'time' as const, label: t('settings.system.sections.time') },
      { value: 'firewall' as const, label: t('settings.system.sections.firewall') },
      { value: 'systemLog' as const, label: t('settings.system.sections.systemLog') }
    ],
    [t]
  );

  const supportedSections = useMemo(
    () => allSections.filter((item) => supportsSystemSection(capabilities, item.value)),
    [allSections, capabilities]
  );

  const sectionSupported = supportedSections.some((item) => item.value === section);
  const sectionWritable = systemSectionWritable(capabilities, section);

  useEffect(() => {
    if (supportedSections.length > 0 && !supportedSections.some((item) => item.value === section)) {
      setSection(supportedSections[0].value);
    }
  }, [supportedSections, section]);

  const updateActionState = useCallback((state: SystemActionState) => {
    setActionState(state);
  }, []);

  useEffect(() => {
    setActionState(idleSystemActionState);
  }, [section]);

  function saveActiveSection() {
    if (!sectionWritable) {
      return;
    }
    if (section === 'network') {
      void networkRef.current?.save();
      return;
    }
    if (section === 'time') {
      void timeRef.current?.save();
      return;
    }
    if (section === 'systemLog') {
      void systemLogRef.current?.save();
    }
  }

  const showSaveAction = sectionWritable && actionState.hasPending;
  const actionStatusColor =
    actionState.statusKind === 'error'
      ? 'text-red-400'
      : actionState.statusKind === 'success'
        ? 'text-green-400'
        : 'text-yellow-400/80';

  return (
    <>
      <div className="sticky top-0 z-20 -mx-1 mb-6 min-w-0 border-b border-neutral-700/70 bg-neutral-900/95 px-1 pb-3 pt-1 shadow-[0_10px_20px_rgba(0,0,0,0.25)] backdrop-blur before:absolute before:bottom-full before:left-0 before:right-0 before:hidden before:h-14 before:bg-neutral-900/95 before:content-[''] sm:top-14 sm:before:block">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="min-w-0 text-base">{t('settings.system.title')}</div>
          <div className="flex min-w-0 flex-col items-stretch gap-2 sm:flex-row sm:flex-wrap sm:items-center sm:justify-end">
            {actionState.statusText && (
              <span className={`min-w-0 truncate text-xs sm:max-w-[220px] ${actionStatusColor}`}>
                {actionState.statusText}
              </span>
            )}
            {showSaveAction && (
              <Button
                type="primary"
                size="small"
                icon={<SaveIcon size={14} />}
                loading={actionState.isBusy}
                disabled={!actionState.canSave}
                onClick={saveActiveSection}
              >
                {t('settings.system.saveChanges')}
              </Button>
            )}
            <div className="min-w-0 max-w-full overflow-x-auto">
              <Segmented
                size="small"
                className="min-w-max"
                value={sectionSupported ? section : undefined}
                options={supportedSections}
                onChange={(value) => setSection(value as SystemSection)}
              />
            </div>
          </div>
        </div>
      </div>

      <div className="min-w-0 overflow-x-hidden">
        {sectionSupported && section === 'network' && (
          <Network ref={networkRef} showTitle={false} onActionStateChange={updateActionState} />
        )}
        {sectionSupported && section === 'time' && (
          <TimeSettings
            ref={timeRef}
            showFooter={false}
            readOnly={!sectionWritable}
            onActionStateChange={updateActionState}
          />
        )}
        {sectionSupported && section === 'firewall' && <FirewallSettings />}
        {sectionSupported && section === 'systemLog' && (
          <SystemLog
            ref={systemLogRef}
            showTitle={false}
            showFooter={false}
            readOnly={!sectionWritable}
            onActionStateChange={updateActionState}
          />
        )}
      </div>
    </>
  );
};
