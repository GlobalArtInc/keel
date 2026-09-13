import { ChangeDetectionStrategy, Component, computed, inject, input } from '@angular/core';
import { KeelI18n } from '@globalart/keel/client';
import type { FieldError } from '@globalart/keel/meta';
import { PhxBannerComponent } from '@globalart/phoenix-ui';

export const VALIDATION_KEY = 'keel.error.validation';

export interface BannerEntry {
  path: string;
  message: string;
}

export function bannerEntryOf(i18n: KeelI18n, finding: FieldError): BannerEntry {
  return {
    path: finding.path,
    message: i18n.t(finding.key ?? finding.code ?? VALIDATION_KEY, finding.params),
  };
}

@Component({
  selector: 'keel-findings',
  imports: [PhxBannerComponent],
  template: `
    <phx-banner status="danger" role="alert">
      @for (entry of entries(); track $index) {
        <div class="banner__entry">
          @if (entry.path) {
            <code class="banner__path">{{ entry.path }}</code>
          }
          <span class="banner__message">{{ entry.message }}</span>
        </div>
      }
    </phx-banner>
  `,
  styles: `
    :host {
      display: block;
    }
    .banner__entry {
      display: flex;
      gap: 8px;
      align-items: baseline;
    }
    .banner__path {
      font: var(--phx-mono-14);
      color: var(--phx-ink-muted);
    }
  `,
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelFindingsComponent {
  readonly findings = input.required<readonly FieldError[]>();

  private readonly i18n = inject(KeelI18n);

  protected readonly entries = computed<BannerEntry[]>(() =>
    this.findings().map((finding) => bannerEntryOf(this.i18n, finding)),
  );
}
