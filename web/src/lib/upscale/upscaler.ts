/**
 * The page's upscaler (docs/design/screen-upscaling-v1.md): draws the
 * captured frame on a canvas through WebGL2, enlarged sharply (a deringing
 * pass at the frame's size, then SGSR 1 to the canvas) or plainly (the GPU's
 * bilinear filter). It takes any frame WebGL can upload: a VideoFrame, a
 * <video>, an <img>, an ImageBitmap or a canvas. Textures and the deringed
 * frame are kept and reused; a frame of a new size reallocates them once. A
 * lost context draws nothing until the browser restores it, and the caller
 * hears of both.
 */
import { BILINEAR, DERING, SGSR, VERTEX } from './shaders.ts';

export type UpscaleSource = TexImageSource;

/** How sharply the picture is enlarged; `plain` is the GPU's bilinear filter. */
export type UpscaleMode = 'plain' | 'low' | 'medium' | 'high';

/** The deringing sigma (in levels) and SGSR's edge sharpness of each mode. */
export const MODES: Record<Exclude<UpscaleMode, 'plain'>, { sigma: number; edge: number }> = {
  low: { sigma: 20, edge: 1.5 },
  medium: { sigma: 16, edge: 2.0 },
  high: { sigma: 12, edge: 2.0 }
};

/** How a view's frames are drawn, for the quality monitor. */
export type UpscaleReport = {
  /**
   * `off`: the view's plain path, without WebGL; `plain`: through WebGL at
   * the frame's own size; or the sharpness the picture is enlarged with.
   */
  mode: 'off' | UpscaleMode;
  /** Medians over the last seconds: a frame's drawing on its thread, and on the GPU where the browser tells. */
  drawMs: number | null;
  gpuMs: number | null;
};

export type DrawOptions = {
  mode: UpscaleMode;
  /** The board's VP8 picture: undo the browser's narrow-range stretch. */
  vp8Range?: boolean;
  /** The part of the frame drawn to the whole canvas, in frame pixels; all of it by default. */
  crop?: { x: number; y: number; width: number; height: number };
};

type Program = { program: WebGLProgram; uniforms: Map<string, WebGLUniformLocation | null> };

type State = {
  vao: WebGLVertexArrayObject;
  source: WebGLTexture;
  sourceSize: [number, number];
  deringed: WebGLTexture;
  deringedFramebuffer: WebGLFramebuffer;
  deringedSize: [number, number];
  dering: Program;
  sgsr: Program;
  bilinear: Program;
};

export type UpscalerEvents = {
  /** The context was lost: nothing is drawn until it comes back. */
  onLost?: () => void;
  /** The context is back and the programs are made again. */
  onRestored?: () => void;
};

function compile(gl: WebGL2RenderingContext, fragment: string, names: string[]): Program {
  const program = gl.createProgram();
  if (!program) throw new Error('no program');
  for (const [type, source] of [
    [gl.VERTEX_SHADER, VERTEX],
    [gl.FRAGMENT_SHADER, fragment]
  ] as const) {
    const shader = gl.createShader(type);
    if (!shader) throw new Error('no shader');
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS) && !gl.isContextLost()) {
      throw new Error(gl.getShaderInfoLog(shader) ?? 'shader');
    }
    gl.attachShader(program, shader);
    gl.deleteShader(shader);
  }
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS) && !gl.isContextLost()) {
    throw new Error(gl.getProgramInfoLog(program) ?? 'link');
  }
  const uniforms = new Map(names.map((name) => [name, gl.getUniformLocation(program, name)]));
  return { program, uniforms };
}

function texture(gl: WebGL2RenderingContext, filter: number): WebGLTexture {
  const made = gl.createTexture();
  if (!made) throw new Error('no texture');
  gl.bindTexture(gl.TEXTURE_2D, made);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filter);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  return made;
}

/** EXT_disjoint_timer_query_webgl2's names this file uses. */
type TimerQuery = { TIME_ELAPSED_EXT: number; GPU_DISJOINT_EXT: number };

export class Upscaler {
  readonly canvas: HTMLCanvasElement | OffscreenCanvas;
  readonly gl: WebGL2RenderingContext;
  private state: State | null = null;
  private readonly lost: (event: Event) => void;
  private readonly restored: () => void;
  /** GPU timer queries where the browser offers them, one frame in flight. */
  private timer: TimerQuery | null = null;
  private query: WebGLQuery | null = null;
  private queryInFlight = false;
  /** The GPU's time of the frames measured since the last `takeGpuTimes`, ms. */
  private gpuTimes: number[] = [];

  /** A WebGL2 upscaler on `canvas`, or null where the browser has no WebGL2. */
  static on(
    canvas: HTMLCanvasElement | OffscreenCanvas,
    events: UpscalerEvents = {}
  ): Upscaler | null {
    const gl = canvas.getContext('webgl2', {
      alpha: false,
      antialias: false,
      depth: false,
      stencil: false,
      premultipliedAlpha: false,
      preserveDrawingBuffer: false
    }) as WebGL2RenderingContext | null;
    return gl ? new Upscaler(canvas, gl, events) : null;
  }

  private constructor(
    canvas: HTMLCanvasElement | OffscreenCanvas,
    gl: WebGL2RenderingContext,
    events: UpscalerEvents
  ) {
    this.canvas = canvas;
    this.gl = gl;
    this.lost = (event: Event) => {
      event.preventDefault();
      this.state = null;
      events.onLost?.();
    };
    this.restored = () => {
      this.state = this.make();
      if (this.state) events.onRestored?.();
    };
    canvas.addEventListener('webglcontextlost', this.lost);
    canvas.addEventListener('webglcontextrestored', this.restored);
    this.state = this.make();
  }

  /** Whether frames can be drawn: false while the context is lost. */
  get ready(): boolean {
    return this.state !== null && !this.gl.isContextLost();
  }

  private make(): State | null {
    const gl = this.gl;
    if (gl.isContextLost()) return null;
    this.timer = gl.getExtension('EXT_disjoint_timer_query_webgl2') as TimerQuery | null;
    this.query = this.timer ? gl.createQuery() : null;
    this.queryInFlight = false;
    const vao = gl.createVertexArray();
    const deringedFramebuffer = gl.createFramebuffer();
    if (!vao || !deringedFramebuffer) return null;
    const output = ['uFrame', 'uFrameSize', 'uCrop', 'uOutSize'];
    return {
      vao,
      source: texture(gl, gl.LINEAR),
      sourceSize: [0, 0],
      deringed: texture(gl, gl.LINEAR),
      deringedFramebuffer,
      deringedSize: [0, 0],
      dering: compile(gl, DERING, ['uSource', 'uSigma', 'uVp8Range']),
      sgsr: compile(gl, SGSR, [...output, 'uEdgeSharpness']),
      bilinear: compile(gl, BILINEAR, output)
    };
  }

  /**
   * Draws `frame` (`width` x `height` pixels) over the whole canvas, its
   * drawing buffer's size. Returns false when nothing was drawn (a lost
   * context, or an empty frame).
   */
  draw(frame: UpscaleSource, width: number, height: number, options: DrawOptions): boolean {
    const gl = this.gl;
    const state = this.state;
    if (!state || gl.isContextLost() || width <= 0 || height <= 0) return false;
    const [outWidth, outHeight] = [gl.drawingBufferWidth, gl.drawingBufferHeight];
    const timing = this.beginTiming();

    // The frame, uploaded into the texture of its size.
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, state.source);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    if (state.sourceSize[0] !== width || state.sourceSize[1] !== height) {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
      state.sourceSize = [width, height];
    }
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.UNSIGNED_BYTE, frame);

    let frameTexture = state.source;
    if (options.mode !== 'plain' || options.vp8Range) {
      // The deringing pass (drawn plainly, only VP8's table: sigma 0), into
      // the deringed frame of the same size.
      if (state.deringedSize[0] !== width || state.deringedSize[1] !== height) {
        gl.bindTexture(gl.TEXTURE_2D, state.deringed);
        gl.texImage2D(
          gl.TEXTURE_2D,
          0,
          gl.RGBA8,
          width,
          height,
          0,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          null
        );
        gl.bindFramebuffer(gl.FRAMEBUFFER, state.deringedFramebuffer);
        gl.framebufferTexture2D(
          gl.FRAMEBUFFER,
          gl.COLOR_ATTACHMENT0,
          gl.TEXTURE_2D,
          state.deringed,
          0
        );
        state.deringedSize = [width, height];
      }
      const { program, uniforms } = state.dering;
      gl.useProgram(program);
      gl.bindFramebuffer(gl.FRAMEBUFFER, state.deringedFramebuffer);
      gl.viewport(0, 0, width, height);
      gl.bindTexture(gl.TEXTURE_2D, state.source);
      gl.uniform1i(uniforms.get('uSource')!, 0);
      const sigma = options.mode === 'plain' ? 0 : MODES[options.mode].sigma;
      gl.uniform1f(uniforms.get('uSigma')!, sigma / 255);
      gl.uniform1i(uniforms.get('uVp8Range')!, options.vp8Range ? 1 : 0);
      gl.bindVertexArray(state.vao);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      frameTexture = state.deringed;
    }

    const { program, uniforms } = options.mode === 'plain' ? state.bilinear : state.sgsr;
    gl.useProgram(program);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.viewport(0, 0, outWidth, outHeight);
    gl.bindTexture(gl.TEXTURE_2D, frameTexture);
    gl.uniform1i(uniforms.get('uFrame')!, 0);
    gl.uniform2f(uniforms.get('uFrameSize')!, width, height);
    const crop = options.crop;
    gl.uniform4f(
      uniforms.get('uCrop')!,
      crop?.x ?? 0,
      crop?.y ?? 0,
      crop?.width ?? width,
      crop?.height ?? height
    );
    gl.uniform2f(uniforms.get('uOutSize')!, outWidth, outHeight);
    if (options.mode !== 'plain')
      gl.uniform1f(uniforms.get('uEdgeSharpness')!, MODES[options.mode].edge);
    gl.bindVertexArray(state.vao);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    if (timing && this.timer) gl.endQuery(this.timer.TIME_ELAPSED_EXT);
    return true;
  }

  /**
   * Starts timing this frame on the GPU when the browser can and no frame
   * is being timed; collects the last frame's time once it is known.
   */
  private beginTiming(): boolean {
    const gl = this.gl;
    const timer = this.timer;
    const query = this.query;
    if (!timer || !query) return false;
    if (this.queryInFlight) {
      if (!gl.getQueryParameter(query, gl.QUERY_RESULT_AVAILABLE)) return false;
      const nanoseconds = gl.getQueryParameter(query, gl.QUERY_RESULT) as number;
      if (!gl.getParameter(timer.GPU_DISJOINT_EXT) && this.gpuTimes.length < 1000) {
        this.gpuTimes.push(nanoseconds / 1e6);
      }
      this.queryInFlight = false;
    }
    gl.beginQuery(timer.TIME_ELAPSED_EXT, query);
    this.queryInFlight = true;
    return true;
  }

  /** The GPU's time of the frames measured since the last call, ms; empty where the browser does not tell. */
  takeGpuTimes(): number[] {
    const times = this.gpuTimes;
    this.gpuTimes = [];
    return times;
  }

  /** Lets go of the GPU's objects and the canvas's events; the canvas keeps its context. */
  dispose(): void {
    const gl = this.gl;
    const state = this.state;
    this.state = null;
    this.canvas.removeEventListener('webglcontextlost', this.lost);
    this.canvas.removeEventListener('webglcontextrestored', this.restored);
    if (!state || gl.isContextLost()) return;
    gl.deleteTexture(state.source);
    gl.deleteTexture(state.deringed);
    gl.deleteFramebuffer(state.deringedFramebuffer);
    gl.deleteVertexArray(state.vao);
    if (this.query) gl.deleteQuery(this.query);
    this.query = null;
    for (const { program } of [state.dering, state.sgsr, state.bilinear]) gl.deleteProgram(program);
  }
}
