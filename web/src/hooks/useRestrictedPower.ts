import { message } from 'antd';
import { useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';

import { supportsPowerControl } from '@/lib/capabilities.ts';
import { performPowerAction, refusalOf } from '@/lib/restricted-power.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';

export function useRestrictedPower() {
  const { t } = useTranslation();
  const manifest = useAtomValue(capabilitiesAtom);
  const restricted = supportsPowerControl(manifest);

  // A press that fails is said, never left as a rejection nobody hears.
  async function run(action: 'power_short' | 'power_long' | 'reset') {
    try {
      if (!manifest) {
        throw new Error('capabilities are not available');
      }
      const result = await performPowerAction(manifest.platform.id, action);
      if (!result.applied) {
        message.warning(result.rejection ?? 'power action was not applied');
      }
      return result;
    } catch (error) {
      const reason = refusalOf(error) ?? (error instanceof Error ? error.message : String(error));
      message.warning(t('power.failed', { reason }));
      return null;
    }
  }

  return { restricted, run };
}
