import { downloadUrl } from './download';

export const PARAMS_PART = 'params';
export const JSON_TYPE = 'application/json';

export type UploadFiles = Readonly<Record<string, readonly File[]>>;

export function uploadUrl(func: string): string {
  return downloadUrl(func);
}

export function multipartOf(params: unknown, files: UploadFiles): FormData {
  const body = new FormData();
  body.append(PARAMS_PART, new Blob([JSON.stringify(params ?? {})], { type: JSON_TYPE }));
  for (const [field, list] of Object.entries(files)) {
    for (const file of list) {
      body.append(field, file, file.name);
    }
  }
  return body;
}
