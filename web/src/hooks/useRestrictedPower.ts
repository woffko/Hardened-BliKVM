import { message } from 'antd';
import { useAtomValue } from 'jotai';

import { supportsPowerControl } from '@/lib/capabilities.ts';
import { performPowerAction } from '@/lib/restricted-power.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';

export function useRestrictedPower() {
  const manifest = useAtomValue(capabilitiesAtom);
  const restricted = supportsPowerControl(manifest);

  async function run(action: 'power_short' | 'power_long' | 'reset') {
    if (!manifest) {
      throw new Error('capabilities are not available');
    }
    const result = await performPowerAction(manifest.platform.id, action);
    if (!result.applied) {
      message.warning(result.rejection ?? 'power action was not applied');
    }
    return result;
  }

  return { restricted, run };
}
