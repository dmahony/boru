import { mediaReadChunk, uploadMedia } from "./media-client";

export type BenchmarkSample = {
  workload: string;
  method: string;
  trial: number;
  elapsed_ms: number;
  throughput_per_s: number;
  item_p50_ms: number;
  item_p95_ms: number;
  completed: number;
  ipc_payload_bytes: number;
  main_thread_long_tasks: number | null;
  max_responsiveness_gap_ms: number;
  unsupported?: string;
};

const percentile = (values: number[], p: number) => [...values].sort((a, b) => a - b)[Math.min(values.length - 1, Math.ceil(values.length * p) - 1)] ?? 0;
const frame = () => new Promise<void>(resolve => requestAnimationFrame(() => resolve()));

async function readHandle(handle: string, type: string) {
  const parts: BlobPart[] = [];
  let offset = 0, bytes = 0;
  for (;;) {
    const chunk = await mediaReadChunk(handle, offset);
    bytes += chunk.byteLength;
    if (chunk.byteLength) parts.push(chunk.slice().buffer as ArrayBuffer);
    offset += chunk.byteLength;
    if (chunk.byteLength < 1024 * 1024) break;
  }
  return { blob: new Blob(parts, { type }), bytes };
}

function observeResponsiveness() {
  let alive = true, last = performance.now(), maxGap = 0, longTasks = 0;
  const tick = () => {
    if (!alive) return;
    const now = performance.now(); maxGap = Math.max(maxGap, now - last); last = now;
    setTimeout(tick, 0);
  };
  setTimeout(tick, 0);
  const longTasksSupported = typeof PerformanceObserver !== "undefined" && PerformanceObserver.supportedEntryTypes?.includes("longtask") === true;
  const observer = longTasksSupported ? new PerformanceObserver(list => { longTasks += list.getEntries().length; }) : null;
  observer?.observe({ type: "longtask" });
  return async () => { await new Promise(resolve => setTimeout(resolve, 30)); alive = false; observer?.disconnect(); return { longTasks: longTasksSupported ? longTasks : null, maxGap: +maxGap.toFixed(2) }; };
}

async function pngFixture() {
  const canvas = document.createElement("canvas"); canvas.width = 320; canvas.height = 180;
  const ctx = canvas.getContext("2d")!;
  const gradient = ctx.createLinearGradient(0, 0, 320, 180); gradient.addColorStop(0, "#223344"); gradient.addColorStop(1, "#55cc99");
  ctx.fillStyle = gradient; ctx.fillRect(0, 0, 320, 180); ctx.fillStyle = "white"; ctx.font = "bold 24px sans-serif"; ctx.fillText("Boru media", 20, 100);
  const blob = await new Promise<Blob>((resolve, reject) => canvas.toBlob(value => value ? resolve(value) : reject(Error("PNG encode failed")), "image/png"));
  return new File([blob], "thumbnail.png", { type: "image/png" });
}

async function videoFixture() {
  const response = await fetch("/media-benchmark.webm", { cache: "no-store" });
  if (!response.ok) throw Error(`video fixture HTTP ${response.status}`);
  const bytes = await response.blob();
  return new File([bytes], "benchmark.webm", { type: "video/webm" });
}

async function imageTrial(host: HTMLElement): Promise<Omit<BenchmarkSample, "trial">> {
  const file = await pngFixture(), times: number[] = [], urls: string[] = []; let ipcBytes = 0;
  const handle = await uploadMedia(file, ({ transferredBytes }) => { ipcBytes = transferredBytes; });
  const stopResponsiveness = observeResponsiveness(), start = performance.now();
  for (let i = 0; i < 24; i++) {
    const itemStart = performance.now(), data = await readHandle(handle.handle, file.type);
    ipcBytes += data.bytes;
    const url = URL.createObjectURL(data.blob), image = new Image(); urls.push(url); image.src = url;
    await image.decode(); host.append(image); times.push(performance.now() - itemStart);
  }
  await frame();
  const elapsed = performance.now() - start, responsive = await stopResponsiveness();
  host.replaceChildren(); urls.forEach(URL.revokeObjectURL);
  return { workload: "still-image thumbnails (24 PNGs)", method: "<img> + real media_read_chunk opaque handle", elapsed_ms: +elapsed.toFixed(2), throughput_per_s: +(24 * 1000 / elapsed).toFixed(2), item_p50_ms: +percentile(times, .5).toFixed(2), item_p95_ms: +percentile(times, .95).toFixed(2), completed: 24, ipc_payload_bytes: ipcBytes, main_thread_long_tasks: responsive.longTasks, max_responsiveness_gap_ms: responsive.maxGap };
}

async function videoTrial(host: HTMLElement): Promise<Omit<BenchmarkSample, "trial">> {
  const file = await videoFixture(); let uploadBytes = 0;
  const handle = await uploadMedia(file, ({ transferredBytes }) => { uploadBytes = transferredBytes; });
  const start = performance.now(), stopResponsiveness = observeResponsiveness(), data = await readHandle(handle.handle, file.type);
  const url = URL.createObjectURL(data.blob), video = document.createElement("video");
  video.muted = true; video.playsInline = true; video.src = url; host.replaceChildren(video);
  await new Promise<void>((resolve, reject) => {
    video.onloadeddata = () => resolve();
    video.onerror = () => reject(Error(`WebM decode failed (code=${video.error?.code ?? "none"}, network=${video.networkState}, ready=${video.readyState}, canPlay=${video.canPlayType("video/webm")})`));
    video.load();
  });
  await video.play();
  await new Promise<void>((resolve, reject) => {
    const timeout = setTimeout(() => reject(Error("video did not reach ended within 5s")), 5000);
    video.onended = () => { clearTimeout(timeout); resolve(); };
  });
  const elapsed = performance.now() - start, responsive = await stopResponsiveness();
  host.replaceChildren(); URL.revokeObjectURL(url);
  return { workload: "encoded video playback (WebM VP8, 320x180@24, 2s)", method: "<video> + real media_read_chunk opaque handle", elapsed_ms: +elapsed.toFixed(2), throughput_per_s: +(data.blob.size * 1000 / elapsed).toFixed(2), item_p50_ms: +elapsed.toFixed(2), item_p95_ms: +elapsed.toFixed(2), completed: data.blob.size, ipc_payload_bytes: uploadBytes + data.bytes, main_thread_long_tasks: responsive.longTasks, max_responsiveness_gap_ms: responsive.maxGap };
}

async function frameTrial(canvas: HTMLCanvasElement): Promise<Omit<BenchmarkSample, "trial">> {
  const file = await pngFixture(), times: number[] = [], context = canvas.getContext("2d", { alpha: false })!; let ipcBytes = 0;
  const handle = await uploadMedia(file, ({ transferredBytes }) => { ipcBytes = transferredBytes; });
  const stopResponsiveness = observeResponsiveness(), start = performance.now();
  for (let i = 0; i < 60; i++) {
    const itemStart = performance.now(), data = await readHandle(handle.handle, file.type);
    ipcBytes += data.bytes;
    const bitmap = await createImageBitmap(data.blob); context.drawImage(bitmap, 0, 0, canvas.width, canvas.height); bitmap.close();
    times.push(performance.now() - itemStart); await frame();
  }
  const elapsed = performance.now() - start, responsive = await stopResponsiveness();
  return { workload: "synthetic decoded frames (60)", method: "Canvas 2D + real media_read_chunk opaque handle", elapsed_ms: +elapsed.toFixed(2), throughput_per_s: +(60 * 1000 / elapsed).toFixed(2), item_p50_ms: +percentile(times, .5).toFixed(2), item_p95_ms: +percentile(times, .95).toFixed(2), completed: 60, ipc_payload_bytes: ipcBytes, main_thread_long_tasks: responsive.longTasks, max_responsiveness_gap_ms: responsive.maxGap };
}

export async function runRealMediaBenchmark(canvas: HTMLCanvasElement, gallery: HTMLElement, video: HTMLElement) {
  const warm = await pngFixture(), handle = await uploadMedia(warm, () => {}); await readHandle(handle.handle, warm.type);
  const samples: BenchmarkSample[] = [];
  for (let trial = 1; trial <= 5; trial++) {
    for (const [name, run] of [["images", () => imageTrial(gallery)], ["video", () => videoTrial(video)], ["frames", () => frameTrial(canvas)]] as const) {
      try { samples.push({ ...await run(), trial }); }
      catch (error) { samples.push({ workload: name, method: "real opaque-handle IPC", trial, elapsed_ms: 0, throughput_per_s: 0, item_p50_ms: 0, item_p95_ms: 0, completed: 0, ipc_payload_bytes: 0, main_thread_long_tasks: 0, max_responsiveness_gap_ms: 0, unsupported: String(error) }); }
    }
  }
  const testCanvas = document.createElement("canvas"), gl = testCanvas.getContext("webgl2") ?? testCanvas.getContext("webgl");
  const debugInfo = gl?.getExtension("WEBGL_debug_renderer_info");
  return { runtime: { user_agent: navigator.userAgent, platform: navigator.platform, device_pixel_ratio: devicePixelRatio, viewport: [innerWidth, innerHeight], longtask_observer_supported: typeof PerformanceObserver !== "undefined" && PerformanceObserver.supportedEntryTypes?.includes("longtask") === true, webgl_vendor: debugInfo && gl ? gl.getParameter(debugInfo.UNMASKED_VENDOR_WEBGL) : null, webgl_renderer: debugInfo && gl ? gl.getParameter(debugInfo.UNMASKED_RENDERER_WEBGL) : null }, samples };
}
