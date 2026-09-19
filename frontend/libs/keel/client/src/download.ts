import type { CallError } from '@globalart/keel/meta';

export const RAW_PREFIX = '/api/v1/x';
export const CONTENT_DISPOSITION = 'Content-Disposition';

export interface DownloadedFile {
  blob: Blob;
  filename: string | null;
}

export type DownloadResult = { ok: true; file: DownloadedFile } | { ok: false; error: CallError };

export function downloadUrl(func: string): string {
  const dot = func.indexOf('.');
  if (dot <= 0 || dot === func.length - 1) {
    throw new TypeError(`${func} does not name a module and a function`);
  }
  return `${RAW_PREFIX}/${encodeURIComponent(func.slice(0, dot))}/${encodeURIComponent(func.slice(dot + 1))}`;
}

export function filenameOf(disposition: string | null): string | null {
  if (disposition === null) {
    return null;
  }
  const encoded = /filename\*=(?:UTF-8|utf-8)''([^;]+)/.exec(disposition);
  if (encoded !== null) {
    try {
      return decodeURIComponent(encoded[1].trim());
    } catch {
      return null;
    }
  }
  const plain = /filename=(?:"([^"]*)"|([^;]+))/.exec(disposition);
  if (plain === null) {
    return null;
  }
  const name = (plain[1] ?? plain[2] ?? '').trim();
  return name === '' ? null : name;
}
