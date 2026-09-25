import { ChangeDetectionStrategy, Component, computed, inject, input, signal } from '@angular/core';
import { KeelClient, KeelDates, KeelI18n } from '@globalart/keel/client';
import type { Descriptor } from '@globalart/keel/meta';
import {
  PhxBadgeComponent,
  PhxBannerComponent,
  PhxButtonComponent,
  PhxLogViewComponent,
  PhxPageHeaderComponent,
  PhxPanelComponent,
  type PhxLogLine,
} from '@globalart/phoenix-ui';
import { EMPTY, Observable, catchError, tap } from 'rxjs';
import {
  STREAM_ENDED_KEY,
  STREAM_FAILED_KEY,
  STREAM_LIVE_KEY,
  STREAM_RECONNECT_KEY,
} from '../messages';

type StreamPhase = 'live' | 'ended' | 'failed';

const LOG_HEIGHT = 480;

@Component({
  selector: 'keel-stream',
  imports: [
    PhxPageHeaderComponent,
    PhxBadgeComponent,
    PhxBannerComponent,
    PhxButtonComponent,
    PhxPanelComponent,
    PhxLogViewComponent,
  ],
  template: `
    <phx-page-header [title]="title()">
      @if (phase() === 'live') {
        <phx-badge phxActions color="success">{{ liveLabel() }}</phx-badge>
      } @else {
        <button
          phxActions
          phxButton
          type="button"
          icon="refresh-cw"
          data-action="reconnect"
          (click)="reconnect()"
        >
          {{ reconnectLabel() }}
        </button>
      }
    </phx-page-header>
    @switch (phase()) {
      @case ('ended') {
        <phx-banner status="info">{{ endedText() }}</phx-banner>
      }
      @case ('failed') {
        <phx-banner status="danger">{{ failedText() }}</phx-banner>
      }
    }
    <phx-panel>
      <phx-log-view [lines]="lines()" [label]="title()" [timeZone]="timeZone()" [height]="height" />
    </phx-panel>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 16px;
    }
  `,
  host: {
    '[attr.data-stream]': 'descriptor().name',
    '[attr.data-phase]': 'phase()',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelStreamComponent {
  readonly descriptor = input.required<Descriptor>();
  readonly params = input<Record<string, unknown>>({});

  private readonly client = inject(KeelClient);
  private readonly i18n = inject(KeelI18n);
  private readonly dates = inject(KeelDates);
  private readonly attempt = signal(0);

  protected readonly phase = signal<StreamPhase>('live');
  protected readonly height = LOG_HEIGHT;

  protected readonly lines = computed<Observable<PhxLogLine>>(() => {
    this.attempt();
    const name = this.descriptor().name;
    const params = this.params();
    return this.client.stream(name, params).pipe(
      tap({
        subscribe: () => this.phase.set('live'),
        complete: () => this.phase.set('ended'),
        error: () => this.phase.set('failed'),
      }),
      catchError(() => EMPTY),
    );
  });

  protected readonly title = computed(() => {
    const title = this.descriptor().ui?.title ?? null;
    return title === null ? this.descriptor().name : this.i18n.t(title);
  });
  protected readonly timeZone = computed(() => this.dates.zone());
  protected readonly liveLabel = computed(() => this.i18n.t(STREAM_LIVE_KEY));
  protected readonly reconnectLabel = computed(() => this.i18n.t(STREAM_RECONNECT_KEY));
  protected readonly endedText = computed(() => this.i18n.t(STREAM_ENDED_KEY));
  protected readonly failedText = computed(() => this.i18n.t(STREAM_FAILED_KEY));

  protected reconnect(): void {
    this.attempt.update((count) => count + 1);
  }
}
