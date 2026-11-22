import { invoke } from "@tauri-apps/api/core";

export interface MediaHandle { handle: string; max_chunk_bytes: number }
export interface MediaChunk { offset: number; bytes: number[]; complete: boolean }
export type MediaErrorCode = "invalid_handle" | "expired" | "unauthorized" | "wrong_type" | "invalid_content_type" | "too_large" | "invalid_chunk" | "cancelled" | "io" | "unknown";
export class MediaTransferError extends Error {
  constructor(readonly code: MediaErrorCode, message: string = code) { super(message); this.name = "MediaTransferError"; }
}
export interface MediaProgress { transferredBytes: number; totalBytes: number }
export type ProgressCallback = (progress: MediaProgress) => void;

function typedError(error: unknown): MediaTransferError {
  const raw = typeof error === "string" ? error : error instanceof Error ? error.message : "unknown";
  const code = raw.toLowerCase().match(/invalid_handle|expired|unauthorized|wrong_type|invalid_content_type|too_large|invalid_chunk|cancelled|io/)?.[0] as MediaErrorCode | undefined;
  return new MediaTransferError(code ?? "unknown", raw);
}
async function call<T>(command: string, args: Record<string, unknown>): Promise<T> {
  try { return await invoke<T>(command, args); } catch (error) { throw typedError(error); }
}
const ALLOWED_CONTENT_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp", "video/mp4", "video/webm", "application/octet-stream"]);
function validContentType(value: string): boolean { return ALLOWED_CONTENT_TYPES.has(value.toLowerCase()); }
const MIN_CHUNK_BYTES = 1;

/** Upload bounded slices; never hands a filesystem path or whole file to IPC. */
export async function uploadMedia(file: File, onProgress: ProgressCallback, signal?: AbortSignal): Promise<{ handle: string; size: number }> {
  if (!validContentType(file.type)) throw new MediaTransferError("invalid_content_type", "Unsupported or missing media content type");
  const session = await call<MediaHandle>("media_begin_upload", { contentType: file.type });
  const chunkSize = Math.min(session.max_chunk_bytes, 1024 * 1024);
  if (!Number.isSafeInteger(chunkSize) || chunkSize < MIN_CHUNK_BYTES) throw new MediaTransferError("invalid_chunk");
  let offset = 0;
  try {
    while (offset < file.size) {
      if (signal?.aborted) throw new DOMException("Transfer cancelled", "AbortError");
      const end = Math.min(offset + chunkSize, file.size);
      const bytes = Array.from(new Uint8Array(await file.slice(offset, end).arrayBuffer()));
      if (signal?.aborted) throw new DOMException("Transfer cancelled", "AbortError");
      offset = await call<number>("media_write_chunk", { handle: session.handle, offset, bytes });
      onProgress({ transferredBytes: offset, totalBytes: file.size });
    }
    const size = await call<number>("media_finish_upload", { handle: session.handle });
    if (size !== file.size) throw new MediaTransferError("io", "Uploaded size mismatch");
    return { handle: session.handle, size };
  } catch (error) {
    try { await call<void>("media_cancel", { handle: session.handle }); } catch { /* expiry or completed cancellation is harmless */ }
    throw error;
  }
}

/** Read into a bounded chunk list then Blob; chunk lifetime remains bounded to the resulting download. */
export async function downloadMedia(handle: string, fileName: string, contentType: string, onProgress: ProgressCallback, signal?: AbortSignal): Promise<void> {
  if (!/^(image|video|audio)\/[a-z0-9.+-]+$/i.test(contentType)) throw new MediaTransferError("invalid_content_type");
  const chunks: BlobPart[] = [];
  let offset = 0;
  try {
    while (true) {
      if (signal?.aborted) throw new DOMException("Transfer cancelled", "AbortError");
      const chunk = await call<MediaChunk>("media_read_chunk", { handle, offset, limit: 1024 * 1024 });
      if (chunk.offset !== offset || chunk.bytes.length > 1024 * 1024 || (!chunk.complete && chunk.bytes.length === 0)) throw new MediaTransferError("invalid_chunk", "Invalid read response");
      const data = new Uint8Array(chunk.bytes);
      chunks.push(data.buffer);
      offset += data.byteLength;
      onProgress({ transferredBytes: offset, totalBytes: chunk.complete ? offset : 0 });
      if (chunk.complete) break;
    }
    if (signal?.aborted) throw new DOMException("Transfer cancelled", "AbortError");
    const url = URL.createObjectURL(new Blob(chunks, { type: contentType }));
    const anchor = document.createElement("a"); anchor.href = url; anchor.download = fileName; anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 60_000);
  } catch (error) {
    try { await call<void>("media_cancel", { handle }); } catch { /* invalid/expired handle */ }
    throw error;
  }
}
