import { useEffect, useRef, useState } from 'react';
import { Spin } from 'antd';
import clsx from 'clsx';
import { useAtom, useAtomValue } from 'jotai';

import { getBaseUrl } from '@/lib/service.ts';
import { mouseStyleAtom } from '@/jotai/mouse.ts';
import { resolutionAtom, videoScaleAtom } from '@/jotai/screen.ts';

import { useScreenFit } from './use-screen-fit.ts';

type ServerMessage =
  | { type: 'offer'; sdp: string }
  | { type: 'started' }
  | { type: 'error'; code: string };

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

// VP8 over WebRTC through the server's closed signaling protocol. The server
// owns the gateway session; this client only answers the offer it receives.
export const Vp8Webrtc = () => {
  const resolution = useAtomValue(resolutionAtom);
  const mouseStyle = useAtomValue(mouseStyleAtom);
  const [videoScale, setVideoScale] = useAtom(videoScaleAtom);
  const [isLoading, setIsLoading] = useState(true);
  const [failure, setFailure] = useState<string | null>(null);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const { containerRef, mediaStyle } = useScreenFit(resolution, videoScale, {
    maxWidth: '100%',
    maxHeight: '100%',
    objectFit: 'scale-down'
  });

  useEffect(() => {
    const socket = new WebSocket(`${getBaseUrl('ws')}/api/stream/webrtc`);
    const peer = new RTCPeerConnection({ iceServers: [] });
    const videoElement = videoRef.current;
    let answered = false;

    const send = (message: object) => {
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
    };

    peer.ontrack = (event) => {
      if (videoElement && event.track.kind === 'video') {
        videoElement.srcObject = new MediaStream([event.track]);
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
        return;
      }
      if (message.type === 'started') {
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
    };

    return () => {
      socket.onmessage = null;
      socket.onclose = null;
      socket.close();
      peer.close();
      if (videoElement) videoElement.srcObject = null;
    };
  }, []);

  useEffect(() => {
    setVideoScale(videoScale);
  }, [videoScale, setVideoScale]);

  return (
    <div ref={containerRef} className="flex h-full w-full items-center justify-center">
      <Spin spinning={isLoading && !failure} tip="Loading" size="large">
        {failure ? (
          <div className="text-sm text-neutral-400" role="alert">
            VP8 stream unavailable ({failure})
          </div>
        ) : (
          <video
            id="screen"
            ref={videoRef}
            className={clsx('block min-h-[480px] min-w-[640px] select-none', mouseStyle)}
            style={mediaStyle}
            muted
            autoPlay
            playsInline
            controls={false}
          />
        )}
      </Spin>
    </div>
  );
};
