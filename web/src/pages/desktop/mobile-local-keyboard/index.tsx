import {
  ClipboardEvent,
  CompositionEvent,
  FormEvent,
  KeyboardEvent,
  useEffect,
  useRef
} from 'react';
import { useAtom, useSetAtom } from 'jotai';

import { paste } from '@/api/hid.ts';
import { textToKeyReports } from '@/lib/hid-text.ts';
import { KeyboardReport } from '@/lib/keyboard.ts';
import * as storage from '@/lib/localstorage.ts';
import { client, MessageEvent } from '@/lib/websocket.ts';
import { isKeyboardEnableAtom, isLocalKeyboardOpenAtom } from '@/jotai/keyboard.ts';
import { useHidLease } from '@/hooks/useHidLease.ts';

import { MOBILE_LOCAL_KEYBOARD_INPUT_ID } from './controls.ts';

const VALID_PASTE_LANGUAGES = new Set(['en', 'de', 'fr', 'ru', 'ko', 'ja']);

function getPasteLanguage() {
  const language = storage.getKeyboardLanguage() || 'en';
  return VALID_PASTE_LANGUAGES.has(language) ? language : 'en';
}

export const MobileLocalKeyboard = () => {
  const [isLocalKeyboardOpen, setIsLocalKeyboardOpen] = useAtom(isLocalKeyboardOpenAtom);
  const setIsKeyboardEnable = useSetAtom(isKeyboardEnableAtom);

  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const keyboardRef = useRef(new KeyboardReport());
  const queueRef = useRef(Promise.resolve());
  // Text of the active IME composition that has already been sent.
  const composingRef = useRef(false);
  const composedRef = useRef('');
  const { restricted, reportHid } = useHidLease();

  useEffect(() => {
    const input = inputRef.current;
    if (!input) return;

    if (isLocalKeyboardOpen) {
      requestAnimationFrame(() => input.focus({ preventScroll: true }));
      return;
    }

    input.blur();
  }, [isLocalKeyboardOpen]);

  function enqueue(action: () => void | Promise<void>) {
    queueRef.current = queueRef.current.then(action, action).catch(() => undefined);
  }

  function sendReport(report: Uint8Array) {
    if (restricted) {
      reportHid({ mode: 'keyboard', data: Array.from(report) });
      return;
    }
    const data = new Uint8Array([MessageEvent.Keyboard, ...report]);
    client.send(data);
  }

  function sendKey(code: string) {
    enqueue(() => {
      const keyboard = keyboardRef.current;
      sendReport(keyboard.keyDown(code));
      sendReport(keyboard.keyUp(code));
      sendReport(keyboard.reset());
    });
  }

  function sendText(text: string) {
    if (!text) return;
    if (restricted) {
      enqueue(() => {
        for (const step of textToKeyReports(text)) {
          reportHid({
            mode: 'keyboard',
            data: [step.modifier, 0, step.usage, 0, 0, 0, 0, 0]
          });
          reportHid({ mode: 'keyboard', data: [0, 0, 0, 0, 0, 0, 0, 0] });
        }
      });
      return;
    }
    enqueue(async () => {
      await paste(text, getPasteLanguage());
    });
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
      sendKey('Backspace');
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
      sendKey('Backspace');
      resetInputValue();
      return;
    }

    if (
      nativeEvent.inputType === 'insertLineBreak' ||
      nativeEvent.inputType === 'insertParagraph'
    ) {
      event.preventDefault();
      sendKey('Enter');
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
      sendKey('Backspace');
      return;
    }

    if (event.key === 'Enter') {
      event.preventDefault();
      sendKey('Enter');
      resetInputValue();
    }
  }

  function handleFocus() {
    setIsLocalKeyboardOpen(true);
    setIsKeyboardEnable(false);
  }

  function handleBlur() {
    composingRef.current = false;
    composedRef.current = '';
    setIsLocalKeyboardOpen(false);
    setIsKeyboardEnable(true);
    resetInputValue();
  }

  return (
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
  );
};
