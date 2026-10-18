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

/**
 * The line: among the ready volumes labelled as the agent's drive, the first
 * `hardened-blikvm-agent.exe` whose SHA-256 is the board's agent's is run,
 * and nothing else is, so an image of that label on LUN 0 (one downloaded,
 * for example) cannot have its own program run (security review of
 * 2026-10-18). 255 characters with the digest: the Run box takes 259.
 */
export function agentStartCommand(sha256: string) {
  if (!/^[0-9a-f]{64}$/.test(sha256)) {
    throw new Error("the board's agent's digest is not known");
  }
  return (
    'powershell -w h -c "[IO.DriveInfo]::GetDrives()|?{$_.IsReady -and ' +
    `$_.VolumeLabel -eq '${AGENT_DRIVE_LABEL}'}|%{$_.Name+'hardened-blikvm-agent.exe'}|` +
    `?{(Get-FileHash $_).Hash -eq '${sha256}'}|select -f 1|%{&$_}"`
  );
}

const R = 0x15;
const ENTER = 0x28;

/** Win+R: the Run box. */
export function runBoxSteps(): KeyStep[] {
  return [{ modifier: getModifierBit('MetaLeft') ?? 0x08, usage: R }];
}

/** The command, then Enter. */
export function commandSteps(command: string): KeyStep[] {
  return [...textToKeyReports(command, { script: 'latin' }), { modifier: 0, usage: ENTER }];
}
