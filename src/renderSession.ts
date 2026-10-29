// Pure IPC/session coordination. No Vue, DOM, USB, or runtime imports.
export interface RenderState {
  sessionId: number | null;
  requestId: string | null;
  phase: "idle" | "starting" | "running" | "stopping";
  active: boolean;
  open: boolean;
  ready: boolean;
  serial: string | null;
}
export interface RenderView {
  state: RenderState | null;
  known: boolean;
  canStart: boolean;
  active: boolean;
  pending: boolean;
  previewSession: number | null;
  message: string;
}
export type RenderInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
export interface DeadlineClock {
  schedule: (fn: () => void, ms: number) => unknown;
  cancel: (handle: unknown) => void;
}
const clock: DeadlineClock = {
  schedule: (fn, ms) => setTimeout(fn, ms),
  cancel: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};
export function withTimeout<T>(promise: Promise<T>, ms: number, label: string, timer = clock): Promise<T> {
  return new Promise((resolve, reject) => {
    const handle = timer.schedule(() => reject(new Error(`${label}超时（${ms} ms 无响应）`)), ms);
    promise.then((value) => { timer.cancel(handle); resolve(value); },
      (error) => { timer.cancel(handle); reject(error); });
  });
}
export function createIntentGuard() {
  let revision = 0;
  let disposed = false;
  return {
    begin: () => ++revision,
    current: (token: number) => !disposed && token === revision,
    invalidate: () => { revision++; },
    dispose: () => { disposed = true; revision++; },
  };
}
// UI removal is sequenced after a cleanup attempt, but must not claim that cleanup completed.
export async function cleanupMissingDevice(options: {
  stop: () => Promise<boolean>;
  current: () => boolean;
  clear: () => void;
}): Promise<boolean> {
  const idle = await options.stop();
  if (options.current()) options.clear();
  return idle;
}
let requestCounter = 0;
function newRequestId() {
  return globalThis.crypto?.randomUUID?.() ?? `render-${Date.now()}-${++requestCounter}`;
}
function validState(value: RenderState): boolean {
  return !!value && ["idle", "starting", "running", "stopping"].includes(value.phase)
    && (value.sessionId === null || (Number.isSafeInteger(value.sessionId) && value.sessionId > 0))
    && (value.requestId === null || typeof value.requestId === "string")
    && typeof value.active === "boolean" && typeof value.open === "boolean" && typeof value.ready === "boolean"
    && (value.serial === null || typeof value.serial === "string")
    && (value.phase !== "idle" || (!value.active && !value.open && !value.ready && value.sessionId === null));
}
export function createRenderSession(options: {
  invoke: RenderInvoke;
  changed?: (view: RenderView) => void;
  timeoutMs?: number;
  queryTimeoutMs?: number;
  clock?: DeadlineClock;
  requestId?: () => string;
}) {
  let generation = 0;
  let disposed = false;
  let state: RenderState | null = null;
  let known = false;
  let wanted = false;
  let requestId: string | null = null;
  let pending = false;
  // An idle sampled before start registers / cancellation is acknowledged is not proof of release.
  let unsettledStart = false;
  let stopAcknowledged = true;
  let message = "正在确认后台状态…";
  let queryFlight: Promise<RenderState> | null = null;
  let queryGeneration = -1;
  let queryExpired = false;
  let stopFlight: Promise<boolean> | null = null;
  const commands = new Map<string, Promise<unknown>>();
  const timer = options.clock ?? clock;
  const timeout = options.timeoutMs ?? 30000;
  const queryTimeout = options.queryTimeoutMs ?? 3000;
  function view(): RenderView {
    const idle = known && state?.phase === "idle" && !unsettledStart && stopAcknowledged;
    return {
      state, known, canStart: !!idle && !pending,
      active: !idle, pending,
      previewSession: known && wanted && state?.phase === "running" && state.active
        && state.requestId === requestId ? state.sessionId : null,
      message,
    };
  }
  function emit() { if (!disposed) options.changed?.(view()); }
  function current(token: number) { return !disposed && token === generation; }
  // Keep the raw invocation in the map until it settles, even if the UI deadline expires.
  function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
    const key = `${name}:${args?.requestId ?? ""}`;
    let raw = commands.get(key);
    if (!raw) {
      raw = Promise.resolve().then(() => options.invoke<T>(name, args));
      commands.set(key, raw);
      void raw.then(() => commands.delete(key), () => commands.delete(key));
    }
    return withTimeout(raw as Promise<T>, queryTimeout, name, timer);
  }
  async function refresh(): Promise<boolean> {
    if (disposed) return false;
    const token = generation;
    if (queryFlight && queryExpired) return false;
    if (!queryFlight) {
      const raw = Promise.resolve().then(() => options.invoke<RenderState>("render_state"));
      queryFlight = raw;
      queryGeneration = token;
      queryExpired = false;
      void raw.then(() => { if (queryFlight === raw) queryFlight = null; },
        () => { if (queryFlight === raw) queryFlight = null; });
    }
    const sampledGeneration = queryGeneration;
    const sampledFlight = queryFlight;
    try {
      const next = await withTimeout(sampledFlight, queryTimeout, "查询后台状态", timer);
      if (!current(token) || sampledGeneration !== token) return false;
      if (!validState(next)) throw new Error("后台返回无效会话状态");
      state = next;
      known = true;
      if (next.phase === "idle" && !unsettledStart && stopAcknowledged) {
        wanted = false;
        requestId = null;
        message = "已停止（后台已确认释放）";
      } else if (!wanted) {
        message = "仍在收尾或等待取消确认，尚未释放；可继续停止";
      } else if (next.requestId === requestId && next.phase === "running") {
        message = "正在运行";
      } else {
        message = "启动中或后台仍忙，等待状态确认…";
      }
      emit();
      return view().canStart;
    } catch (error) {
      if (queryFlight === sampledFlight) queryExpired = true;
      if (current(token)) {
        known = false;
        message = `后台状态未知：${error}。未确认停止，可继续停止`;
        emit();
      }
      return false;
    }
  }
  async function stop(): Promise<boolean> {
    if (disposed) return false;
    if (stopFlight) return stopFlight;
    const token = ++generation;
    wanted = false;
    pending = true;
    known = false;
    stopAcknowledged = false;
    message = "正在取消接管，等待后台确认释放…";
    emit();
    const id = requestId;
    const operation = (async () => {
      try {
        // Issue both before awaiting; no delayed release may later close a new request.
        await Promise.all([
          id ? command<unknown>("render_cancel", { requestId: id }) : Promise.resolve(),
          command<string>("render_release"),
        ]);
        if (!current(token)) return false;
        stopAcknowledged = true;
        unsettledStart = false; // acknowledged cancellation covers even pre-registration starts
      } catch (error) {
        if (current(token)) message = `停止请求尚未确认：${error}。可继续停止`;
      } finally {
        if (current(token)) { pending = false; emit(); }
      }
      if (!current(token)) return false;
      return refresh();
    })();
    stopFlight = operation;
    void operation.then(() => { if (stopFlight === operation) stopFlight = null; },
      () => { if (stopFlight === operation) stopFlight = null; });
    return operation;
  }
  async function start(args: { html: string; serial: string | null; width: number; height: number }): Promise<boolean> {
    if (disposed || !view().canStart) return false;
    const token = ++generation;
    const id = (options.requestId ?? newRequestId)();
    if (!id || id.length > 128) throw new Error("无效启动请求ID");
    requestId = id;
    wanted = true;
    unsettledStart = true;
    pending = true;
    known = false;
    message = "启动中…";
    emit();
    try {
      await withTimeout(Promise.resolve().then(() => options.invoke<string>("render_start", { ...args, requestId: id })),
        timeout, "启动渲染", timer);
      if (!current(token)) return false;
      unsettledStart = false;
      pending = false;
      emit();
      await refresh();
      return current(token) && view().previewSession !== null;
    } catch (error) {
      if (!current(token)) return false;
      message = `启动未完成：${error}；正在定向取消，未确认释放前不能再次启动`;
      emit();
      await stop();
      return false;
    } finally {
      if (current(token)) { pending = false; emit(); }
    }
  }
  return {
    view, refresh, start, stop,
    dispose: () => { disposed = true; generation++; wanted = false; },
  };
}

const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];
export function decodePreviewFrame(buffer: ArrayBuffer, expectedSession: number | null): { sessionId: number; png: Uint8Array } | null {
  if (expectedSession === null || !Number.isSafeInteger(expectedSession) || expectedSession <= 0 || buffer.byteLength < 20) return null;
  const bytes = new Uint8Array(buffer);
  if (bytes[0] !== 74 || bytes[1] !== 68 || bytes[2] !== 80 || bytes[3] !== 49) return null;
  const data = new DataView(buffer);
  const id = data.getUint32(4, true) + data.getUint32(8, true) * 0x100000000;
  if (!Number.isSafeInteger(id) || id !== expectedSession) return null;
  if (!PNG_SIGNATURE.every((byte, index) => bytes[index + 12] === byte)) return null;
  return { sessionId: id, png: bytes.subarray(12) };
}
export function createPreviewRenderer<T extends { close: () => void }>(options: {
  session: () => number | null;
  decode: (png: Uint8Array) => Promise<T>;
  paint: (bitmap: T) => void;
  clear: () => void;
}) {
  let revision = 0;
  let decoding = false;
  let disposed = false;
  return {
    invalidate() { revision++; options.clear(); },
    dispose() { disposed = true; revision++; options.clear(); },
    async draw(buffer: ArrayBuffer): Promise<boolean> {
      if (disposed || decoding) return false;
      const frame = decodePreviewFrame(buffer, options.session());
      if (!frame) return false;
      const token = revision;
      decoding = true;
      let bitmap: T | undefined;
      try {
        bitmap = await options.decode(frame.png);
        if (disposed || token !== revision || frame.sessionId !== options.session()) return false;
        options.paint(bitmap);
        return true;
      } catch { return false; }
      finally { bitmap?.close(); decoding = false; }
    },
  };
}
