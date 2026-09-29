import { useEffect, useState } from 'react';
import type { CapabilityManifest } from '@/types/capabilities.ts';
import type { RuntimeInventory } from '@/types/inventory.ts';
import { Button, Splitter } from 'antd';
import { useAtom, useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';
import { useNavigate } from 'react-router-dom';

import * as authApi from '@/api/auth.ts';
import { getRuntimeInventory } from '@/api/inventory.ts';
import { isAvailable, normalizeVideoMode } from '@/lib/capabilities.ts';
import { removeToken } from '@/lib/cookie.ts';
import { HARDENED_NAME } from '@/lib/hardened.ts';
import * as storage from '@/lib/localstorage.ts';
import { formatRtcOffsetSeconds, formatRtcUnixSeconds } from '@/lib/rtc.ts';
import { formatTemperatureMillidegrees } from '@/lib/thermal.ts';
import { browserSupportsWebRtc } from '@/lib/video-transport.ts';
import { client } from '@/lib/websocket.ts';
import {
  capabilitiesAtom,
  capabilitiesErrorAtom,
  capabilitiesLoadingAtom
} from '@/jotai/capabilities.ts';
import { picoclawChatOpenAtom } from '@/jotai/picoclaw.ts';
import { resolutionAtom, videoModeAtom } from '@/jotai/screen.ts';
import { useIsDesktopLayout } from '@/hooks/useResponsiveLayout.ts';
import { Head } from '@/components/head.tsx';

import { CaptureStatusOverlay, useCaptureStatus } from './capture-status';
import { Keyboard } from './keyboard';
import { Menu } from './menu';
import { MobileLocalKeyboard } from './mobile-local-keyboard';
import { Mouse } from './mouse';
import { Notification } from './notification.tsx';
import { Sidebar as PicoclawSidebar } from './picoclaw';
import { ActionOverlay } from './picoclaw/action-overlay.tsx';
import { Screen } from './screen';
import { VirtualKeyboard } from './virtual-keyboard';

function getVideoMode(capabilities: Parameters<typeof normalizeVideoMode>[0]) {
  const isH264DirectSupported = window.location.protocol === 'https:' && !!window.VideoDecoder;
  const cookieVideoMode = storage.getVideoMode();
  let selected = normalizeVideoMode(capabilities, cookieVideoMode);
  if (selected === 'direct' && !isH264DirectSupported) {
    selected = normalizeVideoMode(capabilities, 'mjpeg');
  }
  if ((selected === 'h264' || selected === 'vp8') && !browserSupportsWebRtc()) {
    selected = normalizeVideoMode(capabilities, 'mjpeg');
  }
  if (cookieVideoMode !== selected) storage.setVideoMode(selected);
  return selected;
}

const DesktopReady = ({ capabilities }: { capabilities: CapabilityManifest }) => {
  const { t } = useTranslation();
  const isDesktopLayout = useIsDesktopLayout();
  const hidAvailable = isAvailable(capabilities.hid.status);
  const controlChannelAvailable =
    hidAvailable || capabilities.video.status.availability === 'read_write';
  const [activeVideoMode, setActiveVideoMode] = useState(() => getVideoMode(capabilities));
  const [picoclawSidebarWidth, setPicoclawSidebarWidth] = useState(420);
  const captureStatus = useCaptureStatus(controlChannelAvailable ? activeVideoMode : '');

  const [videoMode, setVideoMode] = useAtom(videoModeAtom);
  const [resolution, setResolution] = useAtom(resolutionAtom);
  const isPicoclawChatOpen = useAtomValue(picoclawChatOpenAtom);

  useEffect(() => {
    setActiveVideoMode(getVideoMode(capabilities));
  }, [capabilities]);

  useEffect(() => {
    if (controlChannelAvailable) {
      client.connect();
    } else {
      client.close();
    }

    setVideoMode(activeVideoMode);

    const res = storage.getResolution() || { width: 0, height: 0 };
    setResolution(res);

    return () => {
      client.close();
    };
  }, [activeVideoMode, controlChannelAvailable, setResolution, setVideoMode]);

  function handleSplitterResize(sizes: number[]) {
    const nextSidebarWidth = sizes[1];
    if (typeof nextSidebarWidth === 'number' && nextSidebarWidth > 0) {
      setPicoclawSidebarWidth(nextSidebarWidth);
    }
  }

  return (
    <div className="h-[100dvh] w-screen overflow-hidden bg-neutral-950">
      <Head title={t('head.desktop')} />

      {isDesktopLayout && <Notification />}

      {videoMode && resolution && (
        <div className="relative flex h-full min-h-0 w-full min-w-0">
          <Menu />
          <div className="h-full min-h-0 w-full min-w-0">
            <Splitter
              className="h-full w-full"
              style={{ height: '100%', width: '100%' }}
              onResize={handleSplitterResize}
            >
              <Splitter.Panel min="45%">
                <div
                  id="kvm-pointer-surface"
                  className="relative h-full min-h-0 w-full min-w-0 overflow-hidden bg-black"
                >
                  <Screen />
                  {controlChannelAvailable && <CaptureStatusOverlay status={captureStatus} />}
                </div>
              </Splitter.Panel>
              <Splitter.Panel
                size={isDesktopLayout && isPicoclawChatOpen ? picoclawSidebarWidth : 0}
                min={isDesktopLayout && isPicoclawChatOpen ? 340 : 0}
                max="45%"
                resizable={isDesktopLayout && isPicoclawChatOpen}
              >
                {isDesktopLayout && isPicoclawChatOpen ? <PicoclawSidebar /> : null}
              </Splitter.Panel>
            </Splitter>
          </div>
          <ActionOverlay />
          {hidAvailable && <Mouse />}
          {hidAvailable && <Keyboard />}
          {hidAvailable && <MobileLocalKeyboard />}
        </div>
      )}

      {!isDesktopLayout && isPicoclawChatOpen ? (
        <div className="fixed inset-x-0 bottom-0 top-14 z-[980] overflow-hidden bg-[#0d0d0f] shadow-2xl">
          <PicoclawSidebar />
        </div>
      ) : null}

      {hidAvailable && <VirtualKeyboard />}
    </div>
  );
};

const AdapterPending = ({ capabilities }: { capabilities: CapabilityManifest }) => {
  const navigate = useNavigate();
  const [logoutError, setLogoutError] = useState(false);
  const [inventory, setInventory] = useState<RuntimeInventory | null>(null);
  const [inventoryChecked, setInventoryChecked] = useState(false);

  useEffect(() => {
    let cancelled = false;
    getRuntimeInventory()
      .then((response) => {
        if (!cancelled && response.code === 0 && response.data.schemaVersion === 3) {
          setInventory(response.data);
        }
      })
      .catch(() => undefined)
      .finally(() => {
        if (!cancelled) setInventoryChecked(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function logout() {
    setLogoutError(false);
    try {
      const response = await authApi.logout();
      if (response.code !== 0) {
        setLogoutError(true);
        return;
      }
      removeToken();
      navigate('/auth/login', { replace: true });
    } catch {
      setLogoutError(true);
    }
  }

  return (
    <div className="flex h-[100dvh] w-screen items-center justify-center bg-neutral-950 text-neutral-300">
      <Head title="Adapter status" />
      <div className="w-full max-w-lg px-6 text-center">
        <div className="text-xl font-semibold">{HARDENED_NAME}</div>
        <div className="mt-2 text-sm text-neutral-500">
          {capabilities.platform.board} was detected, but the Rust hardware adapters are not exposed
          by this build yet.
        </div>
        <div className="mt-5 rounded border border-neutral-800 bg-neutral-900 p-4 text-left text-sm">
          <div>
            Inventory:{' '}
            {inventory
              ? `live via ${inventory.backend.replace(/_/g, ' ')}`
              : inventoryChecked
                ? 'broker unavailable'
                : 'checking broker…'}
          </div>
          {inventory ? (
            <div className="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-neutral-400">
              <span>KVMD</span>
              <span>{inventory.versions.kvmd || 'unknown'}</span>
              <span>Streamer</span>
              <span>{inventory.versions.streamer || 'unknown'}</span>
              <span>Video source</span>
              <span>{inventory.video.online === true ? 'online' : 'not online'}</span>
              <span>HID</span>
              <span>{inventory.hid.online === true ? 'online' : 'not online'}</span>
              <span>Virtual media</span>
              <span>{inventory.virtualMedia.online === true ? 'online' : 'not online'}</span>
              <span>SoC temperature</span>
              <span>
                {formatTemperatureMillidegrees(inventory.thermal.temperatureMillidegreesCelsius)}
              </span>
              <span>External RTC</span>
              <span>{formatRtcUnixSeconds(inventory.rtc.unixSeconds)}</span>
              <span>RTC offset</span>
              <span>{formatRtcOffsetSeconds(inventory.rtc.offsetFromSystemSeconds)}</span>
            </div>
          ) : (
            <div className="mt-2 text-neutral-500">
              Hardware baseline: {capabilities.video.status.evidence.replace(/_/g, ' ')}
            </div>
          )}
          <div className="mt-2 text-neutral-500">
            Remote video, HID, power, and virtual-media controls remain disabled until their backend
            adapters pass the required gates.
          </div>
        </div>
        <div className="mt-5 flex justify-center gap-3">
          <Button onClick={() => navigate('/auth/password')}>Change password</Button>
          <Button danger onClick={logout}>
            Log out
          </Button>
        </div>
        {logoutError ? <div className="mt-3 text-sm text-red-400">Logout failed</div> : null}
      </div>
    </div>
  );
};

export const Desktop = () => {
  const capabilities = useAtomValue(capabilitiesAtom);
  const isLoading = useAtomValue(capabilitiesLoadingAtom);
  const error = useAtomValue(capabilitiesErrorAtom);

  if (isLoading) {
    return (
      <div className="flex h-[100dvh] w-screen items-center justify-center bg-neutral-950 text-neutral-300">
        <Head title="Device verification" />
        <div className="max-w-md px-6 text-center">
          <div className="text-lg font-semibold">{HARDENED_NAME}</div>
          <div className="mt-2 text-sm text-neutral-500">Loading device capabilities…</div>
        </div>
      </div>
    );
  }

  if (!capabilities || error) {
    return (
      <div className="flex h-[100dvh] w-screen items-center justify-center bg-neutral-950 text-neutral-300">
        <Head title="Controls unavailable" />
        <div className="max-w-md px-6 text-center">
          <div className="text-lg font-semibold">Remote controls are unavailable</div>
          <div className="mt-2 text-sm text-neutral-500">
            The device capability manifest could not be verified. Reload after the server is
            healthy.
          </div>
        </div>
      </div>
    );
  }

  if (!isAvailable(capabilities.video.status)) {
    return <AdapterPending capabilities={capabilities} />;
  }

  return <DesktopReady capabilities={capabilities} />;
};
