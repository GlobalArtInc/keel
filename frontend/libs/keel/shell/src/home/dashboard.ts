import { Injectable, inject } from '@angular/core';
import {
  KeelBoot,
  KeelCallError,
  KeelClient,
  KeelDescriptors,
  KeelI18n,
} from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type {
  BootWidget,
  CallError,
  DashboardLayout,
  Descriptor,
  Envelope,
  StatValue,
} from '@globalart/keel/meta';
import { PREF_SET, describeError } from '@globalart/keel/renderer';
import { PhxToastService } from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';

export const DASHBOARD_LAYOUT = 'dashboard.layout';
export const STATS_SERIES = 'stats.series';
export const SPARKLINE_DAYS = 7;
const SPARKLINE_RESOLUTION = 'hour';
const DAY_MS = 86_400_000;

export interface StatReading {
  value: number;
  unit: string;
  points: number[];
}

export type WidgetState =
  | { phase: 'loading'; widget: BootWidget }
  | { phase: 'ready'; widget: BootWidget; descriptor: Descriptor; stat: StatReading | null }
  | { phase: 'failed'; widget: BootWidget; error: CallError };

export function layoutOf(widgets: readonly BootWidget[]): DashboardLayout {
  return {
    order: widgets.map((widget) => widget.func),
    hidden: widgets.filter((widget) => widget.hidden === true).map((widget) => widget.func),
  };
}

export function reordered(widgets: readonly BootWidget[], order: readonly string[]): BootWidget[] {
  const rank = new Map(order.map((func, index) => [func, index]));
  const placed = widgets.filter((widget) => rank.has(widget.func));
  placed.sort((a, b) => (rank.get(a.func) ?? 0) - (rank.get(b.func) ?? 0));
  return [...placed, ...widgets.filter((widget) => !rank.has(widget.func))];
}

export function withHidden(
  widgets: readonly BootWidget[],
  func: string,
  hidden: boolean,
): BootWidget[] {
  return widgets.map((widget) => (widget.func === func ? { ...widget, hidden } : widget));
}

function pointsOf(data: unknown): number[] {
  const points = typeof data === 'object' && data !== null ? Reflect.get(data, 'points') : null;
  if (!Array.isArray(points)) {
    return [];
  }
  return points.flatMap((point: unknown) => {
    const value = typeof point === 'object' && point !== null ? Reflect.get(point, 'value') : null;
    return typeof value === 'number' ? [value] : [];
  });
}

@Injectable({ providedIn: 'root' })
export class KeelDashboard {
  private readonly boot = inject(KeelBoot);
  private readonly client = inject(KeelClient);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly i18n = inject(KeelI18n);
  private readonly toasts = inject(PhxToastService);

  async load(widget: BootWidget): Promise<WidgetState> {
    try {
      const descriptor = await this.descriptors.get(widget.func);
      const stat = widget.kind === 'stat' ? await this.read(widget.func) : null;
      return { phase: 'ready', widget, descriptor, stat };
    } catch (error: unknown) {
      return {
        phase: 'failed',
        widget,
        error: error instanceof KeelCallError ? error.error : unreachable(error),
      };
    }
  }

  async save(widgets: readonly BootWidget[]): Promise<void> {
    const envelope = await this.call(PREF_SET, {
      key: DASHBOARD_LAYOUT,
      value: layoutOf(widgets),
    });
    if (!envelope.ok) {
      this.toasts.danger(describeError(this.i18n, envelope.error));
      return;
    }
    await this.boot.refresh().catch(() => undefined);
  }

  private async read(func: string): Promise<StatReading> {
    const envelope = await this.call<StatValue>(func, {});
    if (!envelope.ok) {
      throw new KeelCallError(envelope.error);
    }
    const stat = envelope.data;
    return { value: stat.value, unit: stat.unit, points: await this.points(stat) };
  }

  private async points(stat: StatValue): Promise<number[]> {
    const series = stat.series ?? null;
    if (series === null) {
      return [];
    }
    const to = new Date();
    const from = new Date(to.getTime() - SPARKLINE_DAYS * DAY_MS);
    const subject = stat.subject ?? null;
    const envelope = await this.call(STATS_SERIES, {
      key: series,
      ...(subject === null ? {} : { subject }),
      from: from.toISOString(),
      to: to.toISOString(),
      resolution: SPARKLINE_RESOLUTION,
    });
    return envelope.ok ? pointsOf(envelope.data) : [];
  }

  private async call<T>(func: string, params: unknown): Promise<Envelope<T>> {
    try {
      return await firstValueFrom(this.client.call<T>(func, params));
    } catch (error: unknown) {
      return { ok: false, error: unreachable(error) };
    }
  }
}
