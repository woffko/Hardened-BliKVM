import { useEffect, useRef, useState } from 'react';
import { Spin } from 'antd';
import { useAtom, useAtomValue, useSetAtom } from 'jotai';
import { useTranslation } from 'react-i18next';

import * as storage from '@/lib/localstorage.ts';
import { watchPeer } from '@/lib/page-video.ts';
import { getBaseUrl } from '@/lib/service.ts';
import { checkSession } from '@/lib/session-check.ts';
import { drawsWithGpu, VP8_RANGE_FILTER_ID, vp8RangeTableValues } from '@/lib/vp8-range.ts';
import { client } from '@/lib/websocket.ts';
import { mouseStyleAtom } from '@/jotai/mouse.ts';
import {
  resolutionAtom,
  upscaleStoppedAtom,
  videoScaleAtom,
  videoStreamGenerationAtom
} from '@/jotai/screen.ts';
import { useUpscale } from '@/hooks/useUpscale.ts';

import { UpscaledVideo } from './upscaled-video.tsx';
import { useScreenFit } from './use-screen-fit.ts';

type ServerMessage =
  | { type: 'offer'; sdp: string }
  | { type: 'started' }
  | { type: 'error'; code: string };

// The board's VP8 samples are of the full range, which VP8 cannot say: the
// picture is drawn through a table that undoes the browser's stretch, where
// the browser draws with a GPU (an SVG filter on the video, or the
// upscaler's shader when it draws the picture).
const RANGE_TABLE = vp8RangeTableValues();

function parseServerMessage(data: unknown): ServerMessage | null {
  if (typeof data !== 'string' || data.length > 65536) return null;
  try {
    const value = JSON.parse(data) as Record<string, unknown>;
    if (value.type === 'offer' && typeof value.sdp === 'string') {
      return { type: 'offer', sdp: value.sdp };
    }
    if (value.type === 'started') return { type: 'started' };
    if (value.type === 'error' && typeof value.code === 'string') {
      return { type: 'error', code: value.code };
    }
  } catch {
    return null;
  }
  return null;
}

// VP8 or H.264 over WebRTC through the server's closed signaling protocol.
// The server owns the gateway session; this client only answers the offer it
// receives.
export const Vp8Webrtc = ({ codec }: { codec: 'vp8' | 'h264' }) => {
  const { t } = useTranslation();
  const [resolution, setResolution] = useAtom(resolutionAtom);
  const mouseStyle = useAtomValue(mouseStyleAtom);
  const [videoScale, setVideoScale] = useAtom(videoScaleAtom);
  const streamGeneration = useAtomValue(videoStreamGenerationAtom);
  const [isLoading, setIsLoading] = useState(true);
  const [failure, setFailure] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const retries = useRef(0);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [rangeFix] = useState(() => codec === 'vp8' && drawsWithGpu());
  const upscale = useUpscale();
  const setUpscaleStopped = useSetAtom(upscaleStoppedAtom);
  const { containerRef, mediaStyle, drawn, pixelRatio } = useScreenFit(resolution, videoScale, {
    maxWidth: '100%',
    maxHeight: '100%',
    objectFit: 'scale-down'
  });

  useEffect(() => {
    // A VPN or privacy extension can replace RTCPeerConnection; report it
    // instead of letting the page fail.
    let peer: RTCPeerConnection;
    try {
      peer = new RTCPeerConnection({ iceServers: [] });
    } catch {
      setFailure('webrtc_unavailable');
      setIsLoading(false);
      return;
    }
    // The quality monitor reads this connection's statistics while it is open.
    const forgetPeer = watchPeer(peer);
    const query = codec === 'h264' ? '?codec=h264' : '';
    const socket = new WebSocket(`${getBaseUrl('ws')}/api/stream/webrtc${query}`);
    let answered = false;
    let attached: HTMLVideoElement | null = null;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;

    // A lost session (server, gateway or network restart) is negotiated again
    // after a growing pause instead of staying broken until a page reload.
    const scheduleRetry = () => {
      if (retryTimer) return;
      const delay = Math.min(2000 * 2 ** Math.min(retries.current, 3), 10000);
      retryTimer = setTimeout(() => {
        void checkSession().then((valid) => {
          if (!valid) return;
          retries.current += 1;
          setAttempt((current) => current + 1);
        });
      }, delay);
    };
    setFailure(null);
    setIsLoading(true);

    const send = (message: object) => {
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
    };

    peer.ontrack = (event) => {
      // Read the ref here: a failure view may have unmounted the element when
      // this attempt started.
      const target = videoRef.current;
      if (target && event.track.kind === 'video') {
        target.srcObject = new MediaStream([event.track]);
        attached = target;
      }
    };
    peer.onicecandidate = (event) => {
      if (!answered) return;
      send({
        type: 'candidate',
        candidate: event.candidate
          ? {
              candidate: event.candidate.candidate,
              sdpMid: event.candidate.sdpMid,
              sdpMLineIndex: event.candidate.sdpMLineIndex
            }
          : null
      });
    };

    socket.onmessage = async (event) => {
      const message = parseServerMessage(event.data);
      if (!message) {
        setFailure('invalid_message');
        socket.close();
        return;
      }
      if (message.type === 'error') {
        setFailure(message.code);
        scheduleRetry();
        return;
      }
      if (message.type === 'started') {
        retries.current = 0;
        setIsLoading(false);
        return;
      }
      if (answered) return;
      try {
        await peer.setRemoteDescription({ type: 'offer', sdp: message.sdp });
        const answer = await peer.createAnswer();
        await peer.setLocalDescription(answer);
        answered = true;
        send({ type: 'answer', sdp: answer.sdp ?? '' });
      } catch {
        setFailure('negotiation_failed');
        socket.close();
      }
    };
    socket.onclose = () => {
      setIsLoading(false);
      scheduleRetry();
    };
    peer.onconnectionstatechange = () => {
      if (peer.connectionState === 'failed') {
        setFailure('connection_failed');
        socket.close();
      }
    };

    return () => {
      clearTimeout(retryTimer);
      forgetPeer();
      socket.onmessage = null;
      socket.onclose = null;
      peer.onconnectionstatechange = null;
      socket.close();
      peer.close();
      if (attached) attached.srcObject = null;
    };
  }, [attempt, codec]);

  // An applied capture change restarts the encoder; negotiate again once the
  // new stream is flowing instead of keeping a frozen track.
  useEffect(() => {
    if (streamGeneration === 0) return;
    const timer = setTimeout(() => {
      retries.current = 0;
      setAttempt((current) => current + 1);
    }, 2000);
    return () => clearTimeout(timer);
  }, [streamGeneration]);

  useEffect(
    () =>
      client.onReconnect(() => {
        retries.current = 0;
        setAttempt((current) => current + 1);
      }),
    []
  );

  useEffect(() => {
    const scale = storage.getVideoScale();
    if (scale) {
      setVideoScale(scale);
    }
  }, [setVideoScale]);

  // Same arrangement as the reference WebRTC player: the scroll container
  // carries the scale and touch pan/pinch, the status sits above the video.
  return (
    <div className="relative h-full min-h-0 w-full min-w-0 overflow-hidden">
      <div
        ref={containerRef}
        data-kvm-screen-scroll
        className="flex h-full min-h-0 w-full min-w-0 items-start justify-start overflow-auto overscroll-contain bg-black"
      >
        <UpscaledVideo
          videoRef={videoRef}
          upscale={upscale}
          vp8Range={codec === 'vp8'}
          videoStyle={
            rangeFix ? { ...mediaStyle, filter: `url(#${VP8_RANGE_FILTER_ID})` } : mediaStyle
          }
          mediaStyle={mediaStyle}
          drawn={drawn}
          pixelRatio={pixelRatio}
          resolution={resolution}
          mouseStyle={mouseStyle}
          onStopped={setUpscaleStopped}
          onResize={(event) => {
            const { videoWidth: width, videoHeight: height } = event.currentTarget;
            // The decoded frame size is the target's real resolution.
            if (
              width > 0 &&
              height > 0 &&
              (width !== resolution?.width || height !== resolution?.height)
            ) {
              setResolution({ width, height });
              storage.setResolution({ width, height });
            }
          }}
        />
      </div>

      {rangeFix && (
        <svg aria-hidden="true" width="0" height="0" className="pointer-events-none absolute">
          <filter id={VP8_RANGE_FILTER_ID} colorInterpolationFilters="sRGB">
            <feComponentTransfer>
              <feFuncR type="table" tableValues={RANGE_TABLE} />
              <feFuncG type="table" tableValues={RANGE_TABLE} />
              <feFuncB type="table" tableValues={RANGE_TABLE} />
            </feComponentTransfer>
          </filter>
        </svg>
      )}

      {failure ? (
        <div
          className="pointer-events-none absolute inset-0 z-50 flex items-center justify-center text-sm text-neutral-400"
          role="alert"
        >
          {failure === 'webrtc_unavailable'
            ? t('screen.videoWebrtcUnavailable')
            : `${codec === 'h264' ? 'H.264' : 'VP8'} stream unavailable (${failure})`}
        </div>
      ) : (
        isLoading && (
          <div className="pointer-events-none absolute inset-0 z-50 flex items-center justify-center bg-black/30 backdrop-blur-[2px] transition-all duration-300">
            <Spin size="large" />
          </div>
        )
      )}
    </div>
  );
};
