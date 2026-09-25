import {
  ChangeDetectionStrategy,
  Component,
  computed,
  effect,
  inject,
  input,
  linkedSignal,
  signal,
  untracked,
} from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
import { KeelBoot, KeelClient, KeelDates, KeelDescriptors, KeelI18n } from '@globalart/keel/client';
import { KeelPageComponent, bodyOf, buildForm, unreachable } from '@globalart/keel/forms';
import type { CallError, Chart, Descriptor, Envelope, ExportFormat } from '@globalart/keel/meta';
import {
  PhxButtonComponent,
  PhxChartComponent,
  PhxDropdownComponent,
  PhxDropdownItemComponent,
  PhxDropdownTriggerDirective,
  PhxErrorComponent,
  PhxListTableComponent,
  PhxLoaderComponent,
  PhxPageHeaderComponent,
  PhxPanelComponent,
  type PhxChartType,
  type PhxColumn,
} from '@globalart/phoenix-ui';
import {
  type Observable,
  catchError,
  combineLatest,
  from,
  map,
  of,
  startWith,
  switchMap,
} from 'rxjs';
import type { Row } from '../action/runner';
import { toColumn, usableZone, type ColumnTotal } from '../columns';
import { EXPORT_FUNCTION } from '../list/actions';
import { KeelExports, formatKeyOf } from '../list/export';
import { APPLY_KEY, EXPORT_KEY, REPORT_EMPTY_KEY, RETRY_KEY, describeError } from '../messages';
import {
  CHART_TYPES,
  answerOf,
  bandLabel,
  bandTotals,
  chartData,
  chartLabel,
  chartParams,
  type ChartData,
  type ReportAnswer,
} from './report';

type ReportState =
  | { phase: 'loading' }
  | { phase: 'loaded'; answer: ReportAnswer }
  | { phase: 'failed'; error: CallError };

type ChartState =
  { phase: 'loading' } | { phase: 'loaded'; data: unknown } | { phase: 'failed'; error: CallError };

interface BandView {
  id: string;
  label: string;
  columns: PhxColumn<Row>[];
  rows: Row[];
  key: (row: Row) => string | number;
  totals: ColumnTotal[];
  formats: readonly ExportFormat[];
}

interface ChartView {
  id: string;
  label: string;
  type: PhxChartType;
  state: ChartState;
  data: ChartData | null;
  message: string | null;
}

const LOADING: ReportState = { phase: 'loading' };
const CHART_LOADING: ChartState = { phase: 'loading' };
const NO_CHARTS: readonly ChartState[] = [];

@Component({
  selector: 'keel-report',
  imports: [
    PhxPageHeaderComponent,
    PhxPanelComponent,
    PhxButtonComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    PhxListTableComponent,
    PhxChartComponent,
    PhxDropdownComponent,
    PhxDropdownItemComponent,
    PhxDropdownTriggerDirective,
    KeelPageComponent,
  ],
  template: `
    <phx-page-header [title]="title()" />
    @if (form().fields.length > 0) {
      <phx-panel class="params">
        <div class="fields">
          @for (page of form().visiblePages(); track page.id) {
            <keel-page [page]="page" />
          }
        </div>
        <div class="apply">
          <button phxButton="primary" type="button" data-action="apply" (click)="apply()">
            {{ applyLabel() }}
          </button>
        </div>
      </phx-panel>
    }
    @switch (state().phase) {
      @case ('loading') {
        <phx-panel
          ><div class="loading"><phx-loader [size]="24" /></div
        ></phx-panel>
      }
      @case ('failed') {
        <phx-panel>
          <phx-error [text]="message()" [retryLabel]="retryLabel()" (retry)="retry()" />
        </phx-panel>
      }
      @case ('loaded') {
        @for (band of bands(); track band.id) {
          <phx-panel [title]="band.label" [attr.data-band]="band.id">
            @if (band.formats.length > 0 && exportable()) {
              <button
                phxPanelActions
                phxButton="default"
                type="button"
                icon="download"
                data-action="export"
                [phxDropdownTrigger]="formats"
                [disabled]="exporting()"
              >
                {{ exportLabel() }}
              </button>
              <phx-dropdown #formats menu align="end" [label]="exportLabel()">
                @for (format of band.formats; track format) {
                  <phx-dropdown-item
                    [attr.data-format]="format"
                    (select)="formats.select(); export(band, format)"
                  >
                    {{ formatLabel(format) }}
                  </phx-dropdown-item>
                }
              </phx-dropdown>
            }
            <phx-list-table
              [columns]="band.columns"
              [rows]="band.rows"
              [rowKey]="band.key"
              [local]="true"
              [reorderable]="false"
              [label]="band.label"
              [emptyTitle]="emptyLabel()"
            >
              <span phxTableStats class="totals">
                @for (total of band.totals; track total.key) {
                  <span class="total" [attr.data-total]="total.key"
                    >{{ total.label }}: {{ total.value }}</span
                  >
                }
              </span>
            </phx-list-table>
          </phx-panel>
        }
        @if (charts().length > 0) {
          <div class="charts">
            @for (chart of charts(); track chart.id) {
              <phx-panel
                [title]="chart.label"
                [attr.data-chart]="chart.id"
                [attr.data-phase]="chart.state.phase"
              >
                @if (chart.data; as data) {
                  <phx-chart
                    [type]="chart.type"
                    [series]="data.series"
                    [labels]="data.labels"
                    [label]="chart.label"
                    [legend]="chart.type === 'pie'"
                    [emptyText]="emptyLabel()"
                    [locale]="locale()"
                  />
                } @else if (chart.message !== null) {
                  <phx-error
                    [text]="chart.message ?? ''"
                    [retryLabel]="retryLabel()"
                    (retry)="retry()"
                  />
                } @else {
                  <div class="loading"><phx-loader [size]="24" /></div>
                }
              </phx-panel>
            }
          </div>
        }
      }
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 16px;
      min-height: 0;
    }
    .fields {
      display: grid;
      gap: var(--phx-form-row-gap, 28px);
    }
    .apply {
      display: flex;
      justify-content: flex-end;
      margin-top: 16px;
    }
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0;
    }
    .charts {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(min(100%, 420px), 1fr));
      gap: 16px;
    }
    .totals {
      display: contents;
    }
    .total {
      padding: 4px 10px;
      font: var(--phx-text-12-single);
      color: var(--phx-ink-muted);
      background-color: var(--phx-tonal);
      border-radius: var(--phx-radius-full);
    }
  `,
  host: {
    '[attr.data-report]': 'descriptor().name',
    '[attr.data-phase]': 'state().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelReportComponent {
  readonly descriptor = input.required<Descriptor>();
  readonly params = input<Record<string, unknown>>({});

  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly dates = inject(KeelDates);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly exports = inject(KeelExports);
  private readonly attempt = signal(0);
  private readonly applied = linkedSignal(() => this.params());

  protected readonly exporting = signal(false);
  readonly form = computed(() => {
    const principal = this.boot.principal();
    const params = this.params();
    const descriptor = this.descriptor();
    return untracked(() =>
      buildForm(descriptor, {
        params,
        values: params,
        principal: principal === null ? null : { level: principal.level },
        client: this.client,
      }),
    );
  });

  readonly state = toSignal(
    combineLatest([
      toObservable(this.descriptor),
      toObservable(this.applied),
      toObservable(this.attempt),
    ]).pipe(
      switchMap(([descriptor, params]) =>
        this.client.call(descriptor.name, params).pipe(
          map((envelope): ReportState => settle(envelope)),
          catchError((error: unknown) =>
            of<ReportState>({ phase: 'failed', error: unreachable(error) }),
          ),
          startWith(LOADING),
        ),
      ),
    ),
    { initialValue: LOADING },
  );

  private readonly chartStates = toSignal(
    toObservable(this.state).pipe(
      switchMap((state) =>
        state.phase === 'loaded' ? this.fetchCharts(state.answer) : of(NO_CHARTS),
      ),
    ),
    { initialValue: NO_CHARTS },
  );

  protected readonly title = computed(() => {
    const title = this.descriptor().ui?.title ?? null;
    return title === null ? this.descriptor().name : this.i18n.t(title);
  });
  protected readonly locale = computed(() => this.dates.locale());
  protected readonly applyLabel = computed(() => this.i18n.t(APPLY_KEY));
  protected readonly retryLabel = computed(() => this.i18n.t(RETRY_KEY));
  protected readonly emptyLabel = computed(() => this.i18n.t(REPORT_EMPTY_KEY));
  protected readonly exportLabel = computed(() => this.i18n.t(EXPORT_KEY));
  protected readonly message = computed(() => {
    const state = this.state();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : '';
  });
  protected readonly exportable = toSignal(
    toObservable(this.descriptor).pipe(
      switchMap((descriptor) =>
        (descriptor.ui?.bands ?? []).some((band) => (band.formats ?? []).length > 0)
          ? from(this.descriptors.list()).pipe(
              map((index) => index.some((entry) => entry.name === EXPORT_FUNCTION)),
              catchError(() => of(false)),
            )
          : of(false),
      ),
    ),
    { initialValue: false },
  );

  protected readonly bands = computed<BandView[]>(() => {
    const state = this.state();
    if (state.phase !== 'loaded') {
      return [];
    }
    const descriptor = this.descriptor();
    const messages = this.i18n.messages();
    const scope = {
      messages,
      zone: this.dates.zone(),
      locale: this.dates.locale(),
      choices: () => null,
    };
    return (descriptor.ui?.bands ?? []).map((band) => {
      const rows = [...(state.answer.bands[band.id] ?? [])];
      const primary = band.columns.find((column) => column.primary)?.key ?? null;
      return {
        id: band.id,
        label: bandLabel(descriptor, band, messages),
        columns: band.columns.map((column) => toColumn(descriptor, column, scope)),
        rows,
        key: (row: Row) => rowKey(rows, row, primary),
        totals: bandTotals(descriptor, band, rows, scope),
        formats: band.formats ?? [],
      };
    });
  });

  protected readonly charts = computed<ChartView[]>(() => {
    const descriptor = this.descriptor();
    const messages = this.i18n.messages();
    const states = this.chartStates();
    const format = { locale: this.dates.locale(), zone: usableZone(this.dates.zone()) };
    return (descriptor.ui?.charts ?? []).map((chart, index) => {
      const state = states[index] ?? CHART_LOADING;
      const label = chartLabel(descriptor, chart, messages);
      return {
        id: chart.id,
        label,
        type: CHART_TYPES[chart.type],
        state,
        data: state.phase === 'loaded' ? chartData(label, chart.id, state.data, format) : null,
        message: state.phase === 'failed' ? describeError(this.i18n, state.error) : null,
      };
    });
  });

  constructor() {
    effect((onCleanup) => {
      const form = this.form();
      onCleanup(() => form.destroy());
    });
  }

  apply(): void {
    const form = this.form();
    form.group.markAllAsTouched();
    if (form.group.invalid) {
      return;
    }
    this.applied.set(bodyOf(form));
  }

  retry(): void {
    this.attempt.update((count) => count + 1);
  }

  async export(band: BandView, format: ExportFormat): Promise<void> {
    if (this.exporting()) {
      return;
    }
    this.exporting.set(true);
    try {
      await this.exports.run(
        {
          func: this.descriptor().name,
          band: band.id,
          params: this.applied(),
          columns: band.columns.map((column) => column.key),
          format,
        },
        band.label,
      );
    } finally {
      this.exporting.set(false);
    }
  }

  protected formatLabel(format: ExportFormat): string {
    return this.i18n.t(formatKeyOf(format));
  }

  private fetchCharts(answer: ReportAnswer): Observable<readonly ChartState[]> {
    const charts: readonly Chart[] = this.descriptor().ui?.charts ?? [];
    if (charts.length === 0) {
      return of(NO_CHARTS);
    }
    return combineLatest(charts.map((chart) => this.fetchChart(chart, answer)));
  }

  private fetchChart(chart: Chart, answer: ReportAnswer): Observable<ChartState> {
    return this.client.call(chart.series, chartParams(chart, answer)).pipe(
      map((envelope): ChartState =>
        envelope.ok
          ? { phase: 'loaded', data: envelope.data }
          : { phase: 'failed', error: envelope.error },
      ),
      catchError((error: unknown) =>
        of<ChartState>({ phase: 'failed', error: unreachable(error) }),
      ),
      startWith(CHART_LOADING),
    );
  }
}

function settle(envelope: Envelope): ReportState {
  return envelope.ok
    ? { phase: 'loaded', answer: answerOf(envelope.data) }
    : { phase: 'failed', error: envelope.error };
}

function rowKey(rows: readonly Row[], row: Row, primary: string | null): string | number {
  const value = primary === null ? undefined : row[primary];
  if (typeof value === 'string' || typeof value === 'number') {
    return value;
  }
  return rows.indexOf(row);
}
