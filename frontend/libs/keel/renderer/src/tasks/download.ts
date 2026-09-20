import { DOCUMENT } from '@angular/common';
import { Injectable, inject } from '@angular/core';
import {
  KeelClient,
  KeelI18n,
  type DownloadResult,
  type DownloadedFile,
} from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { DownloadLink } from '@globalart/keel/meta';
import { PhxToastService } from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import { describeError } from '../messages';

export const REVOKE_AFTER_MS = 1000;

export function handToBrowser(file: DownloadedFile, doc: Document): void {
  const url = URL.createObjectURL(file.blob);
  const anchor = doc.createElement('a');
  anchor.href = url;
  anchor.download = file.filename ?? '';
  anchor.rel = 'noopener';
  anchor.hidden = true;
  doc.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), REVOKE_AFTER_MS);
}

@Injectable({ providedIn: 'root' })
export class KeelDownloads {
  private readonly client = inject(KeelClient);
  private readonly i18n = inject(KeelI18n);
  private readonly toasts = inject(PhxToastService);
  private readonly document = inject(DOCUMENT);

  async save(link: DownloadLink, title: string): Promise<DownloadResult> {
    const result = await firstValueFrom(this.client.download(link.function, link.params)).catch(
      (error: unknown): DownloadResult => ({ ok: false, error: unreachable(error) }),
    );
    if (!result.ok) {
      this.toasts.danger(describeError(this.i18n, result.error), { title });
    } else if ('file' in result) {
      handToBrowser(result.file, this.document);
    }
    return result;
  }
}
