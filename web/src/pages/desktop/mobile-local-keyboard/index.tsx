import {
  ClipboardEvent,
  CompositionEvent,
  FormEvent,
  KeyboardEvent,
  useCallback,
  useEffect,
  useRef,
  useState
} from 'react';
import { getDefaultStore, useAtom, useAtomValue, useSetAtom } from 'jotai';

import { paste } from '@/api/hid.ts';
import * as targetAgent from '@/api/target-agent.ts';
import { layoutToType, scriptOfLayout, scriptRuns } from '@/lib/agent-typing.ts';
import { textToKeyReports, type TextLayoutState } from '@/lib/hid-text.ts';
import { getKeycode } from '@/lib/keymap.ts';
import * as storage from '@/lib/localstorage.ts';
import {
  DOUBLE_TAP_MS,
  keyReport,
  modifierBits,
  StickyModifiers,
  type ModifierModes
} from '@/lib/sticky-modifiers.ts';
import { client, MessageEvent } from '@/lib/websocket.ts';
import { isKeyboardEnableAtom, isLocalKeyboardOpenAtom } from '@/jotai/keyboard.ts';
import { focusConfirmedAtom, leaseEndedAtom } from '@/jotai/restricted-control.ts';
import { agentKeyboardAtom, agentLayoutSyncAtom, targetAgentAtom } from '@/jotai/target-agent.ts';
import { useHidLease } from '@/hooks/useHidLease.ts';
import { readAgentKeyboard, switchTargetLayout } from '@/hooks/useTargetAgent.ts';

import { MOBILE_LOCAL_KEYBOARD_INPUT_ID } from './controls.ts';
import { KeyHelper } from './key-helper.tsx';

const VALID_PASTE_LANGUAGES = new Set(['en', 'de', 'fr', 'ru', 'ko', 'ja']);
// How long the window in front is given to take the layout the agent asked it
// for before the keys after it are typed.
const LAYOUT_SWITCH_MS = 100;

function getPasteLanguage() {
  const language = storage.getKeyboardLanguage() || 'en';
  return VALID_PASTE_LANGUAGES.has(language) ? language : 'en';
}

export const MobileLocalKeyboard = () => {
  const [isLocalKeyboardOpen, setIsLocalKeyboardOpen] = useAtom(isLocalKeyboardOpenAtom);
  const setIsKeyboardEnable = useSetAtom(isKeyboardEnableAtom);

  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const queueRef = useRef(Promise.resolve());
  // Text of the active IME composition that has already been sent.
  const composingRef = useRef(false);
  const composedRef = useRef('');
  // Punctuation after a Russian word is sent at Russian layout positions.
  const layoutRef = useRef<TextLayoutState>({ script: 'latin' });
  // The modifiers of the extra keys row, held on the target while pressed.
  const modifiersRef = useRef(new StickyModifiers());
  const [modifierModes, setModifierModes] = useState<ModifierModes>({});
  // Releases waiting for a possible second tap, by modifier.
  const releaseTimersRef = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  const { restricted, reportHid, hold, drained } = useHidLease();
  const controlConfirmed = useAtomValue(focusConfirmedAtom);
  const leasesEnded = useAtomValue(leaseEndedAtom);

  useEffect(() => {
    const input = inputRef.current;
    if (!input) return;

    if (isLocalKeyboardOpen) {
      requestAnimationFrame(() => input.focus({ preventScroll: true }));
      return;
    }

    input.blur();
  }, [isLocalKeyboardOpen]);

  // Without control nothing is held on the target any more, nor once a lease
  // ended (the window lost focus, the lease was taken again).
  const letGoHeld = useCallback(() => {
    for (const timer of releaseTimersRef.current.values()) clearTimeout(timer);
    releaseTimersRef.current.clear();
    if (modifiersRef.current.reset()) setModifierModes({});
  }, []);
  useEffect(() => {
    if (restricted && !controlConfirmed) letGoHeld();
  }, [restricted, controlConfirmed, letGoHeld]);
  useEffect(() => {
    if (restricted && leasesEnded > 0) letGoHeld();
  }, [restricted, leasesEnded, letGoHeld]);

  useEffect(() => {
    const timers = releaseTimersRef.current;
    return () => {
      for (const timer of timers.values()) clearTimeout(timer);
    };
  }, []);

  function enqueue(action: () => void | Promise<void>) {
    queueRef.current = queueRef.current.then(action, action).catch(() => undefined);
  }

  function sendReport(report: number[]) {
    if (restricted) {
      reportHid({ mode: 'keyboard', data: report });
      return;
    }
    client.send(new Uint8Array([MessageEvent.Keyboard, ...report]));
  }

  function sendReports(reports: number[][]) {
    enqueue(() => {
      for (const report of reports) sendReport(report);
    });
  }

  function showModifiers() {
    setModifierModes(modifiersRef.current.snapshot());
  }

  // A tap on a modifier of the extra keys row: pressed on the target at
  // once, released after the next key or by another tap (lib/sticky-modifiers).
  function tapModifier(code: string) {
    const modifiers = modifiersRef.current;
    const timers = releaseTimersRef.current;
    const before = modifiers.bits();
    const tap = modifiers.tap(code, performance.now());
    clearTimeout(timers.get(code));
    timers.delete(code);
    if (tap === 'releasing') {
      timers.set(
        code,
        setTimeout(() => {
          timers.delete(code);
          if (!modifiers.release(code)) return;
          showModifiers();
          sendReports([keyReport(modifiers.bits())]);
        }, DOUBLE_TAP_MS)
      );
      return;
    }
    showModifiers();
    if (modifiers.bits() !== before) sendReports([keyReport(modifiers.bits())]);
  }

  function releaseModifiers() {
    for (const timer of releaseTimersRef.current.values()) clearTimeout(timer);
    releaseTimersRef.current.clear();
    if (!modifiersRef.current.reset()) return;
    showModifiers();
    sendReports([keyReport(0)]);
  }

  // A key press with the pressed modifiers and, for a shortcut, its own; the
  // one-shot modifiers are released with the key.
  function pressKey(code: string, shortcut: string[] = []) {
    const usage = getKeycode(code);
    if (usage === undefined) return;
    const modifiers = modifiersRef.current;
    const held = modifiers.bits();
    const down = held | modifierBits(shortcut);
    const reports: number[][] = [];
    // The modifiers of a shortcut go down before its key, as on a keyboard.
    if (down !== held) reports.push(keyReport(down));
    reports.push(keyReport(down, usage));
    if (modifiers.consumeOneShot()) showModifiers();
    reports.push(keyReport(modifiers.bits()));
    sendReports(reports);
  }

  // The target's layout follows the letters typed, when the operator asked
  // for it and the agent is connected (lib/agent-typing): a run of Latin,
  // Cyrillic or Estonian letters is typed in a layout for its letters,
  // switched to once the keys before it are typed. A shortcut (a modifier
  // held) is not text and switches nothing.
  function following() {
    const store = getDefaultStore();
    return store.get(agentLayoutSyncAtom) && Boolean(store.get(targetAgentAtom)?.agent);
  }

  async function switchLayout(layout: targetAgent.AgentLayout) {
    const store = getDefaultStore();
    if (store.get(agentKeyboardAtom)?.current?.id === layout.id) return;
    // The agent's call is made under the lease the keys are typed under.
    await switchTargetLayout(layout, hold, drained);
    await new Promise((resolve) => setTimeout(resolve, LAYOUT_SWITCH_MS));
  }

  // The target's layouts as they are now, when the keyboard opens: the
  // operator may have switched it since the last read.
  function readLayouts() {
    if (!following()) return;
    enqueue(async () => {
      hold();
      await drained();
      await readAgentKeyboard();
    });
  }

  // The text run by run, each typed once the target has its layout; decided
  // when its turn comes, with the layouts read by then.
  async function typeFollowing(text: string) {
    for (const run of scriptRuns(text)) {
      const keyboard = getDefaultStore().get(agentKeyboardAtom);
      const layout = run.script
        ? layoutToType(keyboard?.installed ?? [], keyboard?.current ?? null, run.script)
        : null;
      if (run.script && layout) {
        await switchLayout(layout).catch(() => undefined);
        // The run's punctuation, before its first letter too, is typed for
        // the layout it is typed in.
        layoutRef.current.script = scriptOfLayout(layout);
      }
      for (const step of textToKeyReports(run.text, layoutRef.current)) {
        sendReport(keyReport(step.modifier, step.usage));
        sendReport(keyReport(0));
      }
    }
  }

  function sendText(text: string) {
    if (!text) return;
    const modifiers = modifiersRef.current;
    if (!restricted && modifiers.bits() === 0) {
      enqueue(async () => {
        await paste(text, getPasteLanguage());
      });
      return;
    }
    if (modifiers.bits() === 0 && following()) {
      enqueue(() => typeFollowing(text));
      return;
    }
    typeKeys(text);
  }

  function typeKeys(text: string) {
    const modifiers = modifiersRef.current;
    // Typed as key presses at the key positions of the text, with the pressed
    // modifiers: Ctrl, then "c" on the system keyboard, is Ctrl+C.
    const reports: number[][] = [];
    let released = false;
    for (const step of textToKeyReports(text, layoutRef.current)) {
      reports.push(keyReport(step.modifier | modifiers.bits(), step.usage));
      released = modifiers.consumeOneShot() || released;
      reports.push(keyReport(modifiers.bits()));
    }
    if (released) showModifiers();
    sendReports(reports);
  }

  function resetInputValue() {
    if (inputRef.current) {
      inputRef.current.value = '';
    }
  }

  // Android keyboards compose words: every update carries the whole word so
  // far. Send only the difference to what was already typed (backspaces for
  // removed characters, then the new ones) and leave the field alone until the
  // composition ends; clearing it mid-composition breaks the IME.
  function applyComposition(next: string) {
    const previous = composedRef.current;
    let common = 0;
    while (common < previous.length && common < next.length && previous[common] === next[common]) {
      common += 1;
    }
    for (let index = common; index < previous.length; index += 1) {
      pressKey('Backspace');
    }
    const added = next.slice(common);
    if (added) sendText(added.replace(/\r\n/g, '\n'));
    composedRef.current = next;
  }

  function handleCompositionStart() {
    composingRef.current = true;
    composedRef.current = '';
  }

  function handleCompositionUpdate(event: CompositionEvent<HTMLTextAreaElement>) {
    applyComposition(event.data ?? '');
  }

  function handleCompositionEnd(event: CompositionEvent<HTMLTextAreaElement>) {
    applyComposition(event.data ?? '');
    composingRef.current = false;
    composedRef.current = '';
    // Clear after the browser has committed the composition into the field.
    setTimeout(resetInputValue, 0);
  }

  function handleBeforeInput(event: FormEvent<HTMLTextAreaElement>) {
    const nativeEvent = event.nativeEvent as InputEvent;
    if (composingRef.current || nativeEvent.isComposing) return;
    if (nativeEvent.inputType === 'deleteContentBackward') {
      event.preventDefault();
      pressKey('Backspace');
      resetInputValue();
      return;
    }

    if (
      nativeEvent.inputType === 'insertLineBreak' ||
      nativeEvent.inputType === 'insertParagraph'
    ) {
      event.preventDefault();
      pressKey('Enter');
      resetInputValue();
    }
  }

  function handleInput(event: FormEvent<HTMLTextAreaElement>) {
    // Composed text is sent by the composition handlers.
    if (composingRef.current || (event.nativeEvent as InputEvent).isComposing) return;
    const value = inputRef.current?.value || '';
    if (!value) return;

    sendText(value.replace(/\r\n/g, '\n'));
    resetInputValue();
  }

  function handlePaste(event: ClipboardEvent<HTMLTextAreaElement>) {
    const text = event.clipboardData.getData('text/plain');
    if (!text) return;

    event.preventDefault();
    sendText(text);
    resetInputValue();
  }

  function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (composingRef.current || event.nativeEvent.isComposing) return;
    if (event.key === 'Backspace') {
      event.preventDefault();
      pressKey('Backspace');
      return;
    }

    if (event.key === 'Enter') {
      event.preventDefault();
      pressKey('Enter');
      resetInputValue();
    }
  }

  function handleFocus() {
    setIsLocalKeyboardOpen(true);
    setIsKeyboardEnable(false);
    if (restricted) readLayouts();
  }

  function handleBlur() {
    composingRef.current = false;
    composedRef.current = '';
    releaseModifiers();
    setIsLocalKeyboardOpen(false);
    setIsKeyboardEnable(true);
    resetInputValue();
  }

  return (
    <>
      <textarea
        id={MOBILE_LOCAL_KEYBOARD_INPUT_ID}
        ref={inputRef}
        aria-label="Mobile local keyboard input"
        autoCapitalize="off"
        autoComplete="off"
        autoCorrect="off"
        className="fixed bottom-0 left-0 z-[1200] h-px w-px resize-none opacity-0"
        inputMode="text"
        rows={1}
        spellCheck={false}
        tabIndex={-1}
        onBeforeInput={handleBeforeInput}
        onCompositionStart={handleCompositionStart}
        onCompositionUpdate={handleCompositionUpdate}
        onCompositionEnd={handleCompositionEnd}
        onFocus={handleFocus}
        onInput={handleInput}
        onKeyDown={handleKeyDown}
        onBlur={handleBlur}
        onPaste={handlePaste}
      />
      {isLocalKeyboardOpen && (
        <KeyHelper
          modes={modifierModes}
          isMac={storage.getKeyboardSystem() === 'mac'}
          onModifier={tapModifier}
          onKey={(code) => pressKey(code)}
          onCombo={(modifiers, code) => pressKey(code, modifiers)}
          onClose={() => {
            setIsLocalKeyboardOpen(false);
            inputRef.current?.blur();
          }}
        />
      )}
    </>
  );
};
