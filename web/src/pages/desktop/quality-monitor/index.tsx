import {
  memo,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent
} from 'react';
import clsx from 'clsx';
import { getDefaultStore, useAtom, useAtomValue } from 'jotai';
import { GripVerticalIcon, SquareIcon, XIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { sampleDirect, sampleNothing, sampleQuic, sampleWebRtc } from '@/lib/page-video.ts';
import {
  MIN_HEIGHT,
  MIN_WIDTH,
  type PageSample,
  type QualityInputs,
  type QualityLevel,
  type VideoPath
} from '@/lib/quality-monitor.ts';
import type { FromQualityWorker, QualityView, ToQualityWorker } from '@/lib/quality.worker.ts';
import QualityWorker from '@/lib/quality.worker.ts?worker';
import { getBaseUrl } from '@/lib/service.ts';
import { qualityMonitorAtom } from '@/jotai/quality-monitor.ts';
import { videoModeAtom } from '@/jotai/screen.ts';
import { quicStatusAtom, quicVideoStatsAtom } from '@/jotai/transport.ts';
import { useIsDesktopLayout } from '@/hooks/useResponsiveLayout.ts';

/** The page counts its own picture this often while the monitor is open,
 * and never while it is closed. */
const TICK_MS = 1000;
/** The width the window keeps for its scroll bar in Extended. */
const SCROLL_BAR_PX = 12;
/** Basic's width, by layout. */
const BASIC_WIDTH = { desktop: 264, phone: 196 };
/** Room the window keeps inside the page when it is dragged. */
const KEEP_IN_VIEW_PX = 48;

/** The quality monitor's window, while it is open
 * (docs/design/h264-engine-choice-and-quality-monitor-v1.md). */
export const QualityMonitor = () => {
  const layout = useAtomValue(qualityMonitorAtom);
  return layout.open ? <QualityMonitorWindow /> : null;
};

/**
 * The way the picture comes now, read from what draws it: the QUIC
 * session's canvas, the WebRTC video element, the Direct canvas or the
 * MJPEG image. Read once a second, so that the monitor follows no state of
 * the page between its readings.
 */
function currentPath(videoMode: string | null): VideoPath {
  if (videoMode === 'mjpeg') return 'mjpeg';
  if (videoMode === 'direct') return 'direct';
  return document.querySelector('canvas#screen[data-video-transport="quic"]') ? 'quic' : 'webrtc';
}

async function samplePath(path: VideoPath): Promise<PageSample> {
  switch (path) {
    case 'quic': {
      const store = getDefaultStore();
      return sampleQuic(store.get(quicStatusAtom), store.get(quicVideoStatsAtom));
    }
    case 'webrtc':
      return sampleWebRtc();
    case 'direct':
      return sampleDirect();
    case 'mjpeg':
      return sampleNothing();
  }
}

/** The display's refresh, counted with animation frames over a second. */
function useRefreshRate(measure: boolean): number | null {
  const [hz, setHz] = useState<number | null>(null);
  useEffect(() => {
    if (!measure || hz !== null) return;
    let frames = 0;
    let first = 0;
    let request = 0;
    const step = (at: number) => {
      if (!first) first = at;
      frames += 1;
      if (at - first >= 1000) {
        setHz(Math.round(((frames - 1) * 1000) / (at - first)));
        return;
      }
      request = requestAnimationFrame(step);
    };
    request = requestAnimationFrame(step);
    return () => cancelAnimationFrame(request);
  }, [measure, hz]);
  return hz;
}

/** The text's size of the rows, in CSS pixels. */
const FONT_PX = { desktop: 12, phone: 10 };

/**
 * The rows: drawn on a canvas by the monitor's worker, which asks the board
 * and makes them, once a second when this page sends what it counted of its
 * own picture. The page's main thread only counts and sends; the canvas is
 * the compositor's, so that a second's update costs the page next to
 * nothing. In Extended the canvas scrolls inside the window.
 */
const MonitorBody = memo(
  ({
    extended,
    videoMode,
    isDesktop,
    width
  }: {
    extended: boolean;
    videoMode: string | null;
    isDesktop: boolean;
    width: number;
  }) => {
    const { t } = useTranslation();
    const codec: QualityInputs['codec'] =
      videoMode === 'mjpeg' ? 'mjpeg' : videoMode === 'vp8' ? 'vp8' : 'h264';
    const refreshHz = useRefreshRate(extended);
    const canvasRef = useRef<HTMLCanvasElement>(null);
    const workerRef = useRef<Worker | null>(null);
    // A canvas can be handed to a worker once; a new one is mounted for the
    // next worker.
    const [canvasKey, setCanvasKey] = useState(0);
    const [height, setHeight] = useState(0);

    const view = useMemo<QualityView>(
      () => ({
        level: extended ? 'extended' : 'basic',
        codec,
        width,
        fontPx: isDesktop ? FONT_PX.desktop : FONT_PX.phone,
        scale: window.devicePixelRatio || 1,
        labels: t('qualityMonitor', { returnObjects: true }) as Record<string, string>,
        refreshHz
      }),
      [extended, codec, width, isDesktop, t, refreshHz]
    );
    const viewRef = useRef(view);
    viewRef.current = view;

    useEffect(() => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      let offscreen: OffscreenCanvas;
      try {
        offscreen = canvas.transferControlToOffscreen();
      } catch {
        setCanvasKey((key) => key + 1);
        return;
      }
      const worker = new QualityWorker();
      workerRef.current = worker;
      worker.onmessage = (event: MessageEvent<FromQualityWorker>) => {
        if (event.data.type === 'height') setHeight(event.data.height);
      };
      const start: ToQualityWorker = {
        type: 'start',
        url: `${getBaseUrl('http')}/api/vm/quality`,
        canvas: offscreen,
        view: viewRef.current
      };
      worker.postMessage(start, [offscreen]);
      // Once a second: what the page counted of its own picture, by the way
      // it comes now.
      let stopped = false;
      const tick = async () => {
        const path = currentPath(videoMode);
        const sample = await samplePath(path);
        if (stopped) return;
        const message: ToQualityWorker = { type: 'sample', path, sample };
        worker.postMessage(message);
      };
      void tick();
      const timer = setInterval(() => void tick(), TICK_MS);
      return () => {
        stopped = true;
        clearInterval(timer);
        worker.terminate();
        workerRef.current = null;
      };
    }, [canvasKey, videoMode]);

    useEffect(() => {
      const message: ToQualityWorker = { type: 'view', view };
      workerRef.current?.postMessage(message);
    }, [view]);

    const canvas = (
      <canvas
        key={canvasKey}
        ref={canvasRef}
        data-testid="quality-monitor-rows"
        role="img"
        aria-label={t('qualityMonitor.title')}
        className="block"
        style={{ width, height }}
      />
    );
    return extended ? (
      <div data-testid="quality-monitor-body" className="min-h-0 flex-1 overflow-auto">
        {canvas}
      </div>
    ) : (
      <div data-testid="quality-monitor-body">{canvas}</div>
    );
  }
);
MonitorBody.displayName = 'MonitorBody';

const QualityMonitorWindow = () => {
  const { t } = useTranslation();
  const [layout, setLayout] = useAtom(qualityMonitorAtom);
  const isDesktop = useIsDesktopLayout();
  const videoMode = useAtomValue(videoModeAtom);
  const extended = layout.level === 'extended';

  // Dragged by its header, resized at its corner; kept when let go.
  const [place, setPlace] = useState({ x: layout.x, y: layout.y });
  const [size, setSize] = useState({ width: layout.width, height: layout.height });
  const gesture = useRef<{
    kind: 'move' | 'resize';
    startX: number;
    startY: number;
    from: { a: number; b: number };
  } | null>(null);

  const begin = (kind: 'move' | 'resize') => (event: ReactPointerEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    gesture.current = {
      kind,
      startX: event.clientX,
      startY: event.clientY,
      from: kind === 'move' ? { a: place.x, b: place.y } : { a: size.width, b: size.height }
    };
  };
  const move = (event: ReactPointerEvent<HTMLElement>) => {
    const current = gesture.current;
    if (!current) return;
    const dx = event.clientX - current.startX;
    const dy = event.clientY - current.startY;
    if (current.kind === 'move') {
      setPlace({
        x: Math.min(
          Math.max(0, current.from.a + dx),
          Math.max(0, window.innerWidth - KEEP_IN_VIEW_PX)
        ),
        y: Math.min(
          Math.max(0, current.from.b + dy),
          Math.max(0, window.innerHeight - KEEP_IN_VIEW_PX)
        )
      });
    } else {
      setSize({
        width: Math.min(Math.max(MIN_WIDTH, current.from.a + dx), window.innerWidth),
        height: Math.min(Math.max(MIN_HEIGHT, current.from.b + dy), window.innerHeight)
      });
    }
  };
  const end = () => {
    const current = gesture.current;
    if (!current) return;
    gesture.current = null;
    setLayout(current.kind === 'move' ? { x: place.x, y: place.y } : size);
  };

  const left = Math.min(place.x, Math.max(0, window.innerWidth - KEEP_IN_VIEW_PX));
  const top = Math.min(place.y, Math.max(0, window.innerHeight - KEEP_IN_VIEW_PX));

  const level = (value: QualityLevel, label: string) => (
    <button
      type="button"
      data-qm-level={value}
      aria-pressed={layout.level === value}
      className={clsx(
        'rounded border-0 px-1.5 py-0.5 text-[11px]',
        layout.level === value
          ? 'bg-neutral-600 text-white'
          : 'bg-transparent text-neutral-400 hover:text-white'
      )}
      onClick={() => setLayout({ level: value })}
    >
      {label}
    </button>
  );

  // Nothing here reaches the page's own pointer, wheel or touch handlers.
  const keep = (event: { stopPropagation: () => void }) => event.stopPropagation();

  return (
    <div
      data-testid="quality-monitor"
      data-level={layout.level}
      data-opaque={layout.opaque ? 'true' : 'false'}
      className={clsx(
        'fixed z-[950] flex flex-col overflow-hidden rounded shadow-lg',
        isDesktop ? 'text-xs' : 'text-[10px]',
        layout.opaque ? 'bg-neutral-900' : 'bg-neutral-900/70'
      )}
      style={{
        left,
        top,
        // Its own layer: what changes in it is laid out and painted apart
        // from the picture beneath.
        contain: 'layout paint',
        willChange: 'transform',
        ...(extended
          ? { width: size.width, height: size.height }
          : { width: isDesktop ? BASIC_WIDTH.desktop : BASIC_WIDTH.phone })
      }}
      onPointerDown={keep}
      onMouseDown={keep}
      onWheel={keep}
      onTouchStart={keep}
    >
      <div
        data-testid="quality-monitor-header"
        className="flex shrink-0 cursor-move touch-none select-none items-center gap-1 border-b border-neutral-700/60 px-1 py-0.5"
        onPointerDown={begin('move')}
        onPointerMove={move}
        onPointerUp={end}
        onPointerCancel={end}
      >
        <GripVerticalIcon size={12} className="shrink-0 text-neutral-500" />
        <span className="font-semibold text-neutral-200">QM</span>
        <div
          className="ml-auto flex items-center gap-0.5"
          onPointerDown={(event) => event.stopPropagation()}
        >
          {level('basic', t('qualityMonitor.basic'))}
          {level('extended', t('qualityMonitor.extended'))}
          {extended && (
            <button
              type="button"
              data-testid="quality-monitor-opaque"
              aria-pressed={layout.opaque}
              aria-label={t('qualityMonitor.opaque')}
              title={t('qualityMonitor.opaque')}
              className={clsx(
                'ml-0.5 rounded border-0 p-0.5',
                layout.opaque
                  ? 'bg-neutral-600 text-white'
                  : 'bg-transparent text-neutral-400 hover:text-white'
              )}
              onClick={() => setLayout({ opaque: !layout.opaque })}
            >
              <SquareIcon size={12} fill={layout.opaque ? 'currentColor' : 'none'} />
            </button>
          )}
          <button
            type="button"
            data-testid="quality-monitor-close"
            aria-label={t('qualityMonitor.close')}
            title={t('qualityMonitor.close')}
            className="ml-0.5 rounded border-0 bg-transparent p-0.5 text-neutral-400 hover:bg-neutral-700 hover:text-white"
            onClick={() => setLayout({ open: false })}
          >
            <XIcon size={12} />
          </button>
        </div>
      </div>

      <MonitorBody
        extended={extended}
        videoMode={videoMode}
        isDesktop={isDesktop}
        width={
          extended
            ? Math.max(MIN_WIDTH, size.width) - SCROLL_BAR_PX
            : isDesktop
              ? BASIC_WIDTH.desktop
              : BASIC_WIDTH.phone
        }
      />

      {extended && (
        <div
          data-testid="quality-monitor-resize"
          className="absolute bottom-0 right-0 h-3 w-3 cursor-nwse-resize touch-none border-b-2 border-r-2 border-neutral-500"
          onPointerDown={begin('resize')}
          onPointerMove={move}
          onPointerUp={end}
          onPointerCancel={end}
        />
      )}
    </div>
  );
};
