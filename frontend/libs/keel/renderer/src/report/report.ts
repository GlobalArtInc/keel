import { UTC } from '@globalart/keel/client';
import { isObject } from '@globalart/keel/forms';
import {
  entityPrefix,
  lookup,
  type Band,
  type Chart,
  type ChartType,
  type Descriptor,
  type Messages,
  type Total,
} from '@globalart/keel/meta';
import type { PhxChartSeries, PhxChartType } from '@globalart/phoenix-ui';
import type { Row } from '../action/runner';
import { DAY_FORMAT, MOMENT_FORMAT, type ColumnTotal } from '../columns';

export interface ReportAnswer {
  bands: Readonly<Record<string, readonly Row[]>>;
  charts: Readonly<Record<string, Record<string, unknown>>>;
}

export interface ChartData {
  labels: string[];
  series: PhxChartSeries[];
}

export interface ReportFormat {
  messages: Messages;
  locale: string;
  zone: string;
}

export const CHART_TYPES: Readonly<Record<ChartType, PhxChartType>> = {
  line: 'line',
  bar: 'bar',
  pie: 'pie',
};

const UTC_MIDNIGHT = /T00:00:00(?:\.0+)?(?:Z|\+00:00)$/;

export function answerOf(data: unknown): ReportAnswer {
  const answer = isObject(data) ? data : {};
  const bands: Record<string, Row[]> = {};
  for (const [id, rows] of Object.entries(isObject(answer['bands']) ? answer['bands'] : {})) {
    bands[id] = Array.isArray(rows) ? rows.filter(isObject) : [];
  }
  const charts: Record<string, Record<string, unknown>> = {};
  for (const [id, params] of Object.entries(isObject(answer['charts']) ? answer['charts'] : {})) {
    if (isObject(params)) {
      charts[id] = params;
    }
  }
  return { bands, charts };
}

export function bandLabel(descriptor: Descriptor, band: Band, messages: Messages): string {
  const key = `${entityPrefix(descriptor.name)}.band.${band.id}`;
  return lookup(messages, key) ?? key;
}

export function chartLabel(descriptor: Descriptor, chart: Chart, messages: Messages): string {
  const key = `${entityPrefix(descriptor.name)}.chart.${chart.id}`;
  return lookup(messages, key) ?? key;
}

export function chartParams(chart: Chart, answer: ReportAnswer): Record<string, unknown> {
  return { ...(chart.params ?? {}), ...(answer.charts[chart.id] ?? {}) };
}

export function bandTotals(
  descriptor: Descriptor,
  band: Band,
  rows: readonly Row[],
  format: Pick<ReportFormat, 'messages' | 'locale'>,
): ColumnTotal[] {
  const totals: ColumnTotal[] = [];
  const prefix = entityPrefix(descriptor.name);
  for (const column of band.columns) {
    if (!column.total) {
      continue;
    }
    const key = `${prefix}.column.${column.key}`;
    const values = rows
      .map((row) => row[column.key])
      .filter((value): value is number => typeof value === 'number' && Number.isFinite(value));
    totals.push({
      key: column.key,
      label: lookup(format.messages, key) ?? key,
      value: formatNumber(aggregate(column.total, values, rows.length), format.locale),
    });
  }
  return totals;
}

export function chartData(
  label: string,
  key: string,
  data: unknown,
  format: Pick<ReportFormat, 'locale' | 'zone'>,
): ChartData {
  const points = isObject(data) && Array.isArray(data['points']) ? data['points'] : [];
  const labels: string[] = [];
  const values: number[] = [];
  for (const point of points) {
    if (!isObject(point)) {
      continue;
    }
    const value = point['value'];
    labels.push(pointLabel(point, format));
    values.push(typeof value === 'number' && Number.isFinite(value) ? value : 0);
  }
  return { labels, series: [{ key, label, values }] };
}

function pointLabel(
  point: Record<string, unknown>,
  format: Pick<ReportFormat, 'locale' | 'zone'>,
): string {
  const label = point['label'];
  if (typeof label === 'string') {
    return label;
  }
  const at = point['at'];
  if (typeof at !== 'string') {
    return '';
  }
  const moment = new Date(at);
  if (Number.isNaN(moment.getTime())) {
    return at;
  }
  const day = UTC_MIDNIGHT.test(at);
  const options: Intl.DateTimeFormatOptions = day
    ? { ...DAY_FORMAT, timeZone: UTC }
    : { ...MOMENT_FORMAT, timeZone: format.zone };
  try {
    return new Intl.DateTimeFormat(format.locale, options).format(moment);
  } catch {
    return at;
  }
}

function aggregate(total: Total, values: readonly number[], count: number): number {
  switch (total) {
    case 'count':
      return count;
    case 'sum':
      return values.reduce((sum, value) => sum + value, 0);
    case 'avg':
      return values.length === 0
        ? 0
        : values.reduce((sum, value) => sum + value, 0) / values.length;
    case 'min':
      return values.length === 0 ? 0 : Math.min(...values);
    case 'max':
      return values.length === 0 ? 0 : Math.max(...values);
  }
}

function formatNumber(value: number, locale: string): string {
  try {
    return new Intl.NumberFormat(locale).format(value);
  } catch {
    return String(value);
  }
}
