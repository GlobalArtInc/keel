import {
  ChangeDetectionStrategy,
  Component,
  computed,
  effect,
  inject,
  signal,
  untracked,
} from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { FormsModule } from '@angular/forms';
import {
  KeelDates,
  KeelI18n,
  isActive,
  type TaskDetail,
  type TaskRow,
} from '@globalart/keel/client';
import type { CallError, Id, TaskProgress } from '@globalart/keel/meta';
import { describeError, valueOf } from '@globalart/keel/renderer';
import {
  PHX_DRAWER_DATA,
  PHX_LABELS,
  PhxBadgeComponent,
  PhxButtonComponent,
  PhxCodeFieldComponent,
  PhxDrawerBodyComponent,
  PhxDrawerHeaderComponent,
  PhxEmptyComponent,
  PhxErrorComponent,
  PhxLoaderComponent,
  PhxProgressBarComponent,
  PhxPropListComponent,
  PhxToastService,
  type PhxBadgeColor,
  type PhxProp,
} from '@globalart/phoenix-ui';
import {
  KeelTaskWatch,
  TASKS_EMPTY_KEY,
  TASKS_ID_KEY,
  TASKS_LOG_KEY,
  TASKS_REQUEST_KEY,
  TASKS_TITLE_KEY,
  TASK_COLUMN_PREFIX,
  TASK_STATE_PREFIX,
  functionLabel,
} from './tasks';

export interface TaskDrawerData {
  id: Id | null;
}

export type JobsState =
  | { phase: 'loading' }
  | { phase: 'loaded'; rows: TaskRow[] }
  | { phase: 'failed'; error: CallError };

export type DetailState =
  | { phase: 'loading' }
  | { phase: 'loaded'; detail: TaskDetail }
  | { phase: 'failed'; error: CallError };

export const STATE_COLORS: Readonly<Record<TaskRow['state'], PhxBadgeColor>> = {
  queued: 'default',
  running: 'info',
  done: 'success',
  failed: 'danger',
  cancelled: 'warning',
  interrupted: 'warning',
};

export const LOG_HEIGHT = 240;

@Component({
  selector: 'keel-task-drawer',
  imports: [
    FormsModule,
    PhxDrawerHeaderComponent,
    PhxDrawerBodyComponent,
    PhxButtonComponent,
    PhxBadgeComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    PhxEmptyComponent,
    PhxPropListComponent,
    PhxProgressBarComponent,
    PhxCodeFieldComponent,
  ],
  template: `
    <phx-drawer-header [title]="heading()">
      @if (selected() !== null) {
        <button
          phxButton="transparent"
          class="back"
          icon="arrow-left"
          type="button"
          (click)="back()"
        >
          {{ labels.back }}
        </button>
      }
    </phx-drawer-header>
    <phx-drawer-body>
      @if (selected() === null) {
        @switch (jobs().phase) {
          @case ('loading') {
            <div class="loading"><phx-loader [size]="24" /></div>
          }
          @case ('failed') {
            <phx-error [text]="jobsError()" [retryLabel]="labels.retry" (retry)="reload()" />
          }
          @case ('loaded') {
            @if (rows().length === 0) {
              <phx-empty compact illustration="empty" [title]="emptyTitle()" />
            } @else {
              <ul class="jobs">
                @for (row of rows(); track row.id) {
                  <li>
                    <button
                      type="button"
                      class="job"
                      [attr.data-job]="row.id"
                      (click)="open(row.id)"
                    >
                      <span class="job__main">
                        <span class="job__function">{{ functionOf(row.function) }}</span>
                        <phx-badge [color]="colorOf(row.state)">{{ stateOf(row.state) }}</phx-badge>
                      </span>
                      <span class="job__meta">
                        <span class="job__progress">{{ row.progress }}</span>
                        <time class="job__time" [attr.datetime]="row.createdAt">{{
                          when(row.createdAt)
                        }}</time>
                      </span>
                    </button>
                  </li>
                }
              </ul>
            }
          }
        }
      } @else {
        @switch (detail().phase) {
          @case ('loading') {
            <div class="loading"><phx-loader [size]="24" /></div>
          }
          @case ('failed') {
            <phx-error [text]="detailError()" [retryLabel]="labels.retry" (retry)="reload()" />
          }
          @case ('loaded') {
            <phx-prop-list class="props" [items]="props()" />
            <phx-progress-bar
              class="bar"
              [value]="progressValue() ?? 0"
              [indeterminate]="progressValue() === null && running()"
              [color]="barColor()"
              [label]="heading()"
            />
            <phx-code-field
              class="log"
              [label]="logLabel()"
              [ngModel]="logText()"
              [height]="logHeight"
              lineNumbers
              readonly
            />
            @if (running()) {
              <div class="actions">
                <button
                  phxButton
                  type="button"
                  class="cancel"
                  [loading]="cancelling()"
                  (click)="cancel()"
                >
                  {{ labels.cancel }}
                </button>
              </div>
            }
          }
        }
      }
    </phx-drawer-body>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0;
    }
    .jobs {
      display: flex;
      flex-direction: column;
      gap: 8px;
      padding: 0;
      margin: 0;
      list-style: none;
    }
    .job {
      display: flex;
      flex-direction: column;
      gap: 4px;
      width: 100%;
      padding: 12px;
      font: inherit;
      color: var(--phx-ink);
      text-align: left;
      cursor: pointer;
      background-color: var(--phx-tonal);
      border: 0;
      border-radius: var(--phx-radius-medium);
    }
    .job:hover {
      background-color: var(--phx-tonal-hover, var(--phx-tonal));
    }
    .job__main,
    .job__meta {
      display: flex;
      gap: 8px;
      align-items: center;
      justify-content: space-between;
    }
    .job__function {
      font: var(--phx-heading-14);
      overflow-wrap: anywhere;
    }
    .job__meta {
      font: var(--phx-text-12-single);
      color: var(--phx-ink-muted);
    }
    .props {
      margin-bottom: 16px;
    }
    .bar {
      margin-bottom: 16px;
    }
    .actions {
      display: flex;
      justify-content: flex-end;
      margin-top: 16px;
    }
  `,
  host: {
    '[attr.data-job]': 'selected()',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelTaskDrawerComponent {
  protected readonly labels = inject(PHX_LABELS);
  private readonly data = inject<TaskDrawerData>(PHX_DRAWER_DATA);
  private readonly watch = inject(KeelTaskWatch);
  private readonly i18n = inject(KeelI18n);
  private readonly dates = inject(KeelDates);
  private readonly toasts = inject(PhxToastService);
  private readonly held = signal<JobsState>({ phase: 'loading' });
  private readonly shown = signal<DetailState>({ phase: 'loading' });
  private readonly busy = signal(false);
  private sequence = 0;

  readonly selected = signal<Id | null>(this.data.id);
  readonly jobs = this.held.asReadonly();
  readonly detail = this.shown.asReadonly();
  readonly cancelling = this.busy.asReadonly();

  protected readonly logHeight = LOG_HEIGHT;
  protected readonly heading = computed(() => {
    const state = this.detail();
    return this.selected() !== null && state.phase === 'loaded'
      ? this.functionOf(state.detail.function)
      : this.i18n.t(TASKS_TITLE_KEY);
  });
  protected readonly emptyTitle = computed(() => this.i18n.t(TASKS_EMPTY_KEY));
  protected readonly logLabel = computed(() => this.i18n.t(TASKS_LOG_KEY));
  protected readonly rows = computed(() => {
    const state = this.jobs();
    return state.phase === 'loaded' ? state.rows : [];
  });
  protected readonly jobsError = computed(() => {
    const state = this.jobs();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : '';
  });
  protected readonly detailError = computed(() => {
    const state = this.detail();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : '';
  });
  private readonly loaded = computed(() => {
    const state = this.detail();
    return state.phase === 'loaded' ? state.detail : null;
  });
  protected readonly running = computed(() => {
    const detail = this.loaded();
    return detail !== null && isActive(detail.state);
  });
  protected readonly progressValue = computed(() => {
    const detail = this.loaded();
    return detail === null ? null : valueOf(detail);
  });
  protected readonly barColor = computed(() => {
    const detail = this.loaded();
    switch (detail?.state) {
      case 'done':
        return 'success';
      case 'failed':
        return 'danger';
      case 'cancelled':
      case 'interrupted':
        return 'warning';
      default:
        return 'accent';
    }
  });
  protected readonly logText = computed(() => this.loaded()?.logTail.join('\n') ?? '');
  protected readonly props = computed<PhxProp[]>(() => {
    const detail = this.loaded();
    if (detail === null) {
      return [];
    }
    const column = (key: string): string => this.i18n.t(`${TASK_COLUMN_PREFIX}.${key}`);
    const items: PhxProp[] = [
      { label: this.i18n.t(TASKS_ID_KEY), value: detail.id, mono: true, copyable: true },
      { label: column('function'), value: this.functionOf(detail.function) },
      { label: column('state'), value: this.stateOf(detail.state) },
      { label: column('progress'), value: detail.progress },
      { label: column('createdAt'), value: this.when(detail.createdAt) },
    ];
    const finishedAt = detail.finishedAt ?? null;
    if (finishedAt !== null) {
      items.push({ label: column('finishedAt'), value: this.when(finishedAt) });
    }
    const requestId = detail.requestId ?? null;
    if (requestId !== null) {
      items.push({ label: this.i18n.t(TASKS_REQUEST_KEY), value: requestId, mono: true });
    }
    return items;
  });

  constructor() {
    this.watch.frames$.pipe(takeUntilDestroyed()).subscribe((frame) => this.onFrame(frame));
    effect(() => {
      const id = this.selected();
      untracked(() => (id === null ? this.loadJobs() : this.loadDetail(id)));
    });
  }

  open(id: Id): void {
    this.selected.set(id);
  }

  back(): void {
    this.selected.set(null);
  }

  reload(): void {
    const id = this.selected();
    if (id === null) {
      this.loadJobs();
    } else {
      this.loadDetail(id);
    }
  }

  async cancel(): Promise<void> {
    const id = this.selected();
    if (id === null || this.busy()) {
      return;
    }
    this.busy.set(true);
    try {
      const envelope = await this.watch.cancel(id);
      if (!envelope.ok) {
        this.toasts.danger(describeError(this.i18n, envelope.error));
      }
    } finally {
      this.busy.set(false);
    }
  }

  protected functionOf(func: string): string {
    return functionLabel(this.i18n.messages(), func);
  }

  protected stateOf(state: TaskRow['state']): string {
    return this.i18n.t(`${TASK_STATE_PREFIX}.${state}`);
  }

  protected colorOf(state: TaskRow['state']): PhxBadgeColor {
    return STATE_COLORS[state];
  }

  protected when(iso: string | null): string {
    return iso === null ? '' : this.dates.format(iso);
  }

  private loadJobs(): void {
    const started = ++this.sequence;
    this.held.set({ phase: 'loading' });
    void this.watch.list().then((envelope) => {
      if (started !== this.sequence) {
        return;
      }
      this.held.set(
        envelope.ok
          ? { phase: 'loaded', rows: envelope.data.data }
          : { phase: 'failed', error: envelope.error },
      );
    });
  }

  private loadDetail(id: Id, quietly = false): void {
    const started = ++this.sequence;
    if (!quietly) {
      this.shown.set({ phase: 'loading' });
    }
    void this.watch.detail(id).then((envelope) => {
      if (started !== this.sequence) {
        return;
      }
      this.shown.set(
        envelope.ok
          ? { phase: 'loaded', detail: envelope.data }
          : { phase: 'failed', error: envelope.error },
      );
    });
  }

  private onFrame(frame: TaskProgress): void {
    const id = this.selected();
    if (id === frame.id) {
      this.loadDetail(id, true);
      return;
    }
    if (id !== null) {
      return;
    }
    const state = this.held();
    if (state.phase !== 'loaded' || !state.rows.some((row) => row.id === frame.id)) {
      this.loadJobs();
      return;
    }
    this.held.set({
      phase: 'loaded',
      rows: state.rows.map((row) =>
        row.id === frame.id
          ? {
              ...row,
              state: frame.state,
              done: frame.done,
              total: frame.total ?? null,
              message: frame.message ?? null,
              progress: progressOf(frame),
            }
          : row,
      ),
    });
  }
}

export function progressOf(frame: Pick<TaskProgress, 'done' | 'total'>): string {
  const total = frame.total ?? null;
  return total === null ? String(frame.done) : `${frame.done}/${total}`;
}
