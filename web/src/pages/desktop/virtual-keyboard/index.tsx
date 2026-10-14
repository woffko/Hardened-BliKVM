import type { ComponentType } from 'react';
import { useEffect, useRef, useState } from 'react';
import { AppleOutlined, WindowsOutlined } from '@ant-design/icons';
import clsx from 'clsx';
import { useAtom, useSetAtom } from 'jotai';
import { XIcon } from 'lucide-react';
import type { KeyboardButtonTheme } from 'react-simple-keyboard';
import * as SimpleKeyboard from 'react-simple-keyboard';
import { Drawer } from 'vaul';

import 'react-simple-keyboard/build/css/index.css';
import '@/assets/styles/keyboard.css';

import { ConfigProvider, Segmented, Select, theme } from 'antd';

import { getKeycode, getModifierBit } from '@/lib/keymap.ts';
import * as storage from '@/lib/localstorage.ts';
import { client, MessageEvent } from '@/lib/websocket.ts';
import { isKeyboardOpenAtom, virtualKeyboardHeightAtom } from '@/jotai/keyboard.ts';
import { useAgentLayoutSync } from '@/hooks/useAgentLayoutSync.ts';
import { useHidLease } from '@/hooks/useHidLease.ts';
import { useReportHeight } from '@/hooks/useKeyboardInsets.ts';
import { useIsDesktopLayout } from '@/hooks/useResponsiveLayout.ts';

import {
  doubleKeys,
  keyboardArrowsOptions,
  keyboardControlPadOptions,
  keyboardOptions,
  modifierKeys,
  specialKeyMap
} from './virtual-keys.ts';

const SimpleKeyboardModule = SimpleKeyboard as Record<string, any>;
const SimpleKeyboardDefault = SimpleKeyboardModule.default as Record<string, any> | undefined;
const SimpleKeyboardComponent = [
  SimpleKeyboardModule.KeyboardReact,
  SimpleKeyboardModule.ReactSimpleKeyboard,
  SimpleKeyboardDefault?.KeyboardReact,
  SimpleKeyboardDefault?.ReactSimpleKeyboard,
  SimpleKeyboardDefault?.default,
  SimpleKeyboardModule['module.exports']?.KeyboardReact,
  SimpleKeyboardModule['module.exports']?.default
].find((candidate) => typeof candidate === 'function') as ComponentType<any>;

const KEYBOARD_LANGUAGES = [
  { value: 'en', label: 'English' },
  { value: 'fr', label: 'French' },
  { value: 'de', label: 'German' },
  { value: 'ru', label: 'Russian' },
  { value: 'ko', label: 'Korean' },
  { value: 'ja', label: 'Japanese' }
];

export const VirtualKeyboard = () => {
  const isDesktopLayout = useIsDesktopLayout();

  const [isKeyboardOpen, setIsKeyboardOpen] = useAtom(isKeyboardOpenAtom);

  const [keyboardLayout, setKeyboardLayout] = useState('default');
  const [keyboardSystem, setKeyboardSystem] = useState('win');
  const [keyboardLanguage, setKeyboardLanguage] = useState('en');
  const [activeModifierKeys, setActiveModifierKeys] = useState<string[]>([]);
  const { restricted, reportHid } = useHidLease();
  // The target's layout follows the keyboard's language when asked to.
  useAgentLayoutSync(keyboardLanguage);

  const keyboardRef = useRef<any>(null);
  // On the touch layout the keyboard covers the bottom of the page; its
  // height lets the screen be laid out above it.
  const touchKeyboardRef = useRef<HTMLDivElement | null>(null);
  const setTouchKeyboardHeight = useSetAtom(virtualKeyboardHeightAtom);
  useReportHeight(touchKeyboardRef, !isDesktopLayout && isKeyboardOpen, setTouchKeyboardHeight);

  const systems = [
    { value: 'win', icon: <WindowsOutlined /> },
    { value: 'mac', icon: <AppleOutlined /> }
  ];

  useEffect(() => {
    const system = storage.getKeyboardSystem();
    if (system && ['win', 'mac'].includes(system)) {
      setKeyboardSystem(system);
    }

    const language = storage.getKeyboardLanguage();
    if (language && KEYBOARD_LANGUAGES.some((item) => item.value === language)) {
      setKeyboardLanguage(language);
    }
  }, []);

  useEffect(() => {
    const layoutMap = new Map([
      ['en', 'default'],
      ['ru', 'rus'],
      ['de', 'qwertz'],
      ['fr', 'azerty'],
      ['ko', 'ko'],
      ['ja', 'ja']
    ]);

    if (keyboardLanguage === 'en' && keyboardSystem === 'mac') {
      setKeyboardLayout('mac');
      return;
    }

    if (layoutMap.has(keyboardLanguage)) {
      setKeyboardLayout(layoutMap.get(keyboardLanguage)!);
      return;
    }

    setKeyboardLayout('default');
  }, [keyboardSystem, keyboardLanguage]);

  // Press key
  function onKeyPress(key: string) {
    if (modifierKeys.includes(key)) {
      if (activeModifierKeys.includes(key)) {
        sendModifierKeyDown();
        sendModifierKeyUp();
      } else {
        setActiveModifierKeys([...activeModifierKeys, key]);
      }
      return;
    }

    sendKeydown(key);
  }

  // Release key
  function onKeyReleased(key: string) {
    if (modifierKeys.includes(key)) {
      return;
    }

    sendKeyup();
  }

  // Send all keys
  function sendKeydown(key: string) {
    const code = getKeyboardCode(key);
    if (!code) {
      console.log('unknown code: ', key);
      return;
    }

    const modifier = sendModifierKeyDown();

    send(modifier, code);
  }

  function getKeyboardCode(key: string) {
    // AZERTY: swap A↔Q and Z↔W on French physical positions
    if (keyboardLanguage === 'fr' && key.endsWith('_azerty')) {
      const base = key.replace('_azerty', '');
      if (base === 'KeyA') return getKeycode('KeyQ');
      if (base === 'KeyQ') return getKeycode('KeyA');
      if (base === 'KeyZ') return getKeycode('KeyW');
      if (base === 'KeyW') return getKeycode('KeyZ');
      // all other labels use their own code
      return getKeycode(base);
    }

    if (keyboardLanguage === 'de' && key.endsWith('_qwertz')) {
      const base = key.replace('_qwertz', '');
      // Tausch
      if (base === 'KeyZ') return getKeycode('KeyY');
      if (base === 'KeyY') return getKeycode('KeyZ');
      // all other labels use their own code
      return getKeycode(base);
    }

    if (keyboardLanguage === 'ko' && key.endsWith('_ko')) {
      const base = key.replace('_ko', '');
      return getKeycode(base);
    }

    if (keyboardLanguage === 'ja' && key.endsWith('_ja')) {
      const base = key.replace('_ja', '');
      return getKeycode(base);
    }

    const specialKey = specialKeyMap.get(key);
    if (specialKey) {
      return getKeycode(specialKey);
    }

    return getKeycode(key);
  }

  // Release all keys
  function sendKeyup() {
    sendModifierKeyUp();
    send(0, 0);
  }

  // Send modifier keys
  function sendModifierKeyDown() {
    let modifier = 0;

    activeModifierKeys.forEach((modifierKey) => {
      const key = specialKeyMap.get(modifierKey)!;

      modifier |= getModifierBit(key)!;
      const code = getKeycode(key)!;

      send(modifier, code);
    });

    return modifier;
  }

  // Release modifier keys
  function sendModifierKeyUp() {
    if (activeModifierKeys.length === 0) return;

    activeModifierKeys.forEach(() => {
      send(0, 0);
    });

    setActiveModifierKeys([]);
  }

  function send(modifier: number, code: number) {
    if (restricted) {
      reportHid({ mode: 'keyboard', data: [modifier, 0, code, 0, 0, 0, 0, 0] });
      return;
    }
    const data = new Uint8Array([MessageEvent.Keyboard, modifier, 0, code, 0, 0, 0, 0, 0]);
    client.send(data);
  }

  function selectSystem(system: string) {
    setKeyboardSystem(system);
    storage.setKeyboardSystem(system);
  }

  function selectLanguage(language: string) {
    setKeyboardLanguage(language);
    storage.setKeyboardLanguage(language);
  }

  function getButtonTheme(): KeyboardButtonTheme[] {
    const theme = [{ class: 'hg-double', buttons: doubleKeys.join(' ') }];

    if (activeModifierKeys.length > 0) {
      const buttons = activeModifierKeys.join(' ');
      theme.push({ class: 'hg-highlight', buttons });
    }

    return theme;
  }

  function getButtonLabel(key: string) {
    return keyboardOptions.display[key as keyof typeof keyboardOptions.display] || key;
  }

  function getMobileButtonClass(key: string) {
    const specialWidth = new Map([
      ['{space}', 'min-w-[132px]'],
      ['{backspace}', 'min-w-[92px]'],
      ['{capslock}', 'min-w-[78px]'],
      ['{shiftleft}', 'min-w-[84px]'],
      ['{shiftright}', 'min-w-[84px]'],
      ['{controlleft}', 'min-w-[68px]'],
      ['{controlright}', 'min-w-[68px]'],
      ['{altleft}', 'min-w-[58px]'],
      ['{altright}', 'min-w-[58px]'],
      ['{enter}', 'min-w-[76px]'],
      ['{tab}', 'min-w-[64px]'],
      ['{winleft}', 'min-w-[58px]'],
      ['{winright}', 'min-w-[58px]'],
      ['{menu}', 'min-w-[58px]']
    ]);

    return specialWidth.get(key) || 'min-w-[42px]';
  }

  function handleMobilePointerUp(key: string) {
    if (!modifierKeys.includes(key)) {
      onKeyReleased(key);
    }
  }

  const mobileRows =
    keyboardOptions.layout[keyboardLayout as keyof typeof keyboardOptions.layout] ||
    keyboardOptions.layout.default;

  if (!isDesktopLayout) {
    if (!isKeyboardOpen) return null;

    return (
      <div
        ref={touchKeyboardRef}
        data-testid="touch-virtual-keyboard"
        className="theme-fixed fixed inset-x-0 bottom-0 z-[1200] max-h-[68dvh] overflow-hidden rounded-t bg-white text-neutral-900 shadow-2xl"
      >
        <div className="keyboard-header flex items-center justify-between gap-3 px-3 py-2">
          <div className="flex min-w-0 items-center gap-2">
            <select
              className="h-8 rounded border border-neutral-300 bg-white px-2 text-sm"
              value={keyboardLanguage}
              onChange={(event) => selectLanguage(event.target.value)}
            >
              {KEYBOARD_LANGUAGES.map((language) => (
                <option key={language.value} value={language.value}>
                  {language.label}
                </option>
              ))}
            </select>

            {keyboardLanguage === 'en' && (
              <div className="flex overflow-hidden rounded border border-neutral-300 text-sm">
                {systems.map((system) => (
                  <button
                    key={system.value}
                    type="button"
                    className={clsx(
                      'flex h-8 min-w-10 items-center justify-center px-3',
                      keyboardSystem === system.value ? 'bg-neutral-800 text-white' : 'bg-white'
                    )}
                    onClick={() => selectSystem(system.value)}
                  >
                    {system.icon}
                  </button>
                ))}
              </div>
            )}
          </div>

          <button
            type="button"
            className="flex h-8 w-8 shrink-0 items-center justify-center rounded text-neutral-600 hover:bg-neutral-200"
            onClick={() => setIsKeyboardOpen(false)}
          >
            <XIcon size={18} />
          </button>
        </div>

        <div className="h-px bg-neutral-300" />

        <div className="max-h-[calc(68dvh-49px)] overflow-auto bg-neutral-200 px-2 py-2">
          <div className="flex min-w-max flex-col gap-1.5">
            {mobileRows.map((row) => (
              <div key={row} className="flex gap-1.5">
                {row.split(' ').map((key) => (
                  <button
                    key={`${row}-${key}`}
                    type="button"
                    className={clsx(
                      'flex h-10 select-none items-center justify-center rounded bg-white px-2 text-center text-sm shadow-sm active:bg-blue-100',
                      getMobileButtonClass(key),
                      activeModifierKeys.includes(key) && 'bg-blue-600 text-white'
                    )}
                    onPointerDown={(event) => {
                      event.preventDefault();
                      onKeyPress(key);
                    }}
                    onPointerUp={() => handleMobilePointerUp(key)}
                    onPointerCancel={() => handleMobilePointerUp(key)}
                    onPointerLeave={() => handleMobilePointerUp(key)}
                    dangerouslySetInnerHTML={{ __html: getButtonLabel(key) }}
                  />
                ))}
              </div>
            ))}
          </div>
        </div>
      </div>
    );
  }

  return (
    <Drawer.Root open={isKeyboardOpen} onOpenChange={setIsKeyboardOpen} modal={false}>
      <Drawer.Portal>
        <Drawer.Content
          className={clsx(
            'theme-fixed max-w-screen fixed bottom-0 left-0 right-0 z-[999] mx-auto overflow-hidden bg-white outline-none',
            isDesktopLayout ? 'w-[820px] rounded' : 'w-screen rounded-t'
          )}
        >
          {/* header */}
          <div className="keyboard-header flex items-center justify-between gap-3 px-3 py-1">
            <ConfigProvider
              theme={{
                algorithm: theme.defaultAlgorithm
              }}
            >
              <div className="flex min-w-0 items-center gap-2 sm:gap-5">
                <Select
                  size="small"
                  className="min-w-[90px]"
                  defaultValue={keyboardLanguage}
                  options={KEYBOARD_LANGUAGES}
                  onChange={selectLanguage}
                />

                {keyboardLanguage === 'en' && (
                  <Segmented
                    size="small"
                    options={systems}
                    value={keyboardSystem}
                    onChange={selectSystem}
                  />
                )}
              </div>
            </ConfigProvider>

            <div className="flex shrink-0 items-center justify-end sm:w-[100px]">
              <div
                className="flex h-[20px] w-[20px] cursor-pointer items-center justify-center rounded text-neutral-600 hover:bg-neutral-300 hover:text-white"
                onClick={() => setIsKeyboardOpen(false)}
              >
                <XIcon size={18} />
              </div>
            </div>
          </div>

          <div className="h-px flex-shrink-0 border-b bg-neutral-300" />

          <div data-vaul-no-drag className="keyboardContainer w-full">
            {/* main keyboard */}
            <SimpleKeyboardComponent
              buttonTheme={getButtonTheme()}
              keyboardRef={(r: any) => (keyboardRef.current = r)}
              onKeyPress={onKeyPress}
              onKeyReleased={onKeyReleased}
              layoutName={keyboardLayout}
              {...keyboardOptions}
            />

            {/* control keyboard */}
            {isDesktopLayout && (
              <div className="controlArrows">
                <SimpleKeyboardComponent
                  onKeyPress={onKeyPress}
                  onKeyReleased={onKeyReleased}
                  {...keyboardControlPadOptions}
                />

                <SimpleKeyboardComponent
                  onKeyPress={onKeyPress}
                  onKeyReleased={onKeyReleased}
                  {...keyboardArrowsOptions}
                />
              </div>
            )}
          </div>
        </Drawer.Content>
        <Drawer.Overlay />
      </Drawer.Portal>
    </Drawer.Root>
  );
};
