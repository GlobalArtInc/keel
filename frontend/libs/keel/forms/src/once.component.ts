import { ChangeDetectionStrategy, Component, computed, inject, input } from '@angular/core';
import { KeelI18n } from '@globalart/keel/client';
import { PhxBannerComponent, PhxPropListComponent, type PhxProp } from '@globalart/phoenix-ui';
import type { OnceRow } from './once';

export const ONCE_TITLE_KEY = 'keel.once.title';

@Component({
  selector: 'keel-once',
  imports: [PhxBannerComponent, PhxPropListComponent],
  template: `
    <phx-banner status="warning" icon="key-round" [title]="title()" role="alert">
      <phx-prop-list [items]="items()" [label]="title()" stacked />
    </phx-banner>
  `,
  styles: `
    :host {
      display: block;
    }
  `,
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelOnceComponent {
  readonly rows = input.required<readonly OnceRow[]>();

  private readonly i18n = inject(KeelI18n);

  protected readonly title = computed(() => this.i18n.t(ONCE_TITLE_KEY));
  protected readonly items = computed<PhxProp[]>(() =>
    this.rows().map((row) => ({
      label: this.i18n.t(row.labelKey),
      value: row.value,
      copyable: true,
      mono: true,
    })),
  );
}
