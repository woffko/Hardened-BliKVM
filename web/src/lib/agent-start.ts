/**
 * "Start the agent" (docs/design/target-agent-v1.md): the keyboard opens
 * Windows' Run box and types a PowerShell line that finds the agent's drive
 * by its label and starts the agent from it, its window hidden. The line is
 * typed at the US layout's positions: the target's layout in front must be
 * English, which the page says before it starts.
 */
import { textToKeyReports, type KeyStep } from './hid-text.ts';
import { getModifierBit } from './keymap.ts';

/** The agent's drive's label (a FAT label has at most 11 characters). */
export const AGENT_DRIVE_LABEL = 'BLIKVMAGENT';

export const AGENT_START_COMMAND =
  'powershell -w h -c "[IO.DriveInfo]::GetDrives()|?{$_.IsReady -and ' +
  `$_.VolumeLabel -eq '${AGENT_DRIVE_LABEL}'}|%{&($_.Name+'hardened-blikvm-agent.exe')}"`;

const R = 0x15;
const ENTER = 0x28;

/** Win+R: the Run box. */
export function runBoxSteps(): KeyStep[] {
  return [{ modifier: getModifierBit('MetaLeft') ?? 0x08, usage: R }];
}

/** The command, then Enter. */
export function commandSteps(command: string = AGENT_START_COMMAND): KeyStep[] {
  return [...textToKeyReports(command, { script: 'latin' }), { modifier: 0, usage: ENTER }];
}
