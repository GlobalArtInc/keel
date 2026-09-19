import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { Router } from '@angular/router';
import {
  KeelDates,
  KeelI18n,
  KeelNotifications,
  KeelProblems,
  PROBLEM_LIST,
  problemKeyOf,
  type Note,
  type Problem,
} from '@globalart/keel/client';
import type { Envelope, NotificationLevel } from '@globalart/keel/meta';
import { SCREEN_PATH, SEARCH_PARAM, describeError } from '@globalart/keel/renderer';
import {
  PHX_DRAWER_DATA,
  PHX_LABELS,
  PhxBadgeComponent,
  PhxDrawerRef,
  PhxNotificationCenterComponent,
  PhxToastService,
  type PhxBadgeColor,
  type PhxNotification,
  type PhxStatus,
} from '@globalart/phoenix-ui';

export const NOTIFICATIONS_TITLE_KEY = 'keel.shell.notifications.title';
export const NOTIFICATIONS_UNREAD_KEY = 'keel.shell.notifications.unread';
export const NOTIFICATIONS_DISMISS_KEY = 'keel.shell.notifications.dismiss';
export const NOTIFICATIONS_CLEAR_KEY = 'keel.shell.notifications.clear';
export const NOTIFICATIONS_PROBLEMS_KEY = 'keel.shell.notifications.problems';
export const NOTIFICATIONS_NO_PROBLEMS_KEY = 'keel.shell.notifications.no_problems';
export const NOTIFICATIONS_SEEN_KEY = 'keel.shell.notifications.seen';
export const LEVEL_PREFIX = 'notify.message.level';
export const SEVERITY_PREFIX = 'notify.problem.severity';

export const LEVEL_STATUS: Readonly<Record<NotificationLevel, PhxStatus>> = {
  info: 'info',
  warning: 'warning',
  error: 'danger',
};

export const SEVERITY_COLORS: Readonly<Record<Problem['severity'], PhxBadgeColor>> = {
  warning: 'warning',
  critical: 'danger',
};

export interface NotificationView extends PhxNotification {
  data: Note;
}

export interface NotificationCentreData {
  problems: boolean;
}

@Component({
  selector: 'keel-notification-centre',
  imports: [PhxNotificationCenterComponent, PhxBadgeComponent],
  template: `
    <section class="centre" [attr.aria-label]="title()">
      <phx-notification-center
        [items]="items()"
        [title]="title()"
        [locale]="locale()"
        markReadOnSelect
        [clearLabel]="clearLabel()"
        [dismissLabel]="dismissLabel()"
        [unreadLabel]="unreadLabel()"
        [yesterdayLabel]="labels.yesterday"
        [emptyTitle]="labels.noNotifications"
        (read)="read($event)"
        (readAll)="readAll()"
        (dismiss)="dismiss($event)"
        (clear)="clear()"
        (select)="select($event)"
      />
    </section>
    @if (data.problems) {
      <section class="problems" data-problems>
        <h3 class="problems__title">{{ problemsTitle() }}</h3>
        @if (problems.problems().length === 0) {
          <p class="problems__empty">{{ noProblems() }}</p>
        } @else {
          <ul class="problems__list">
            @for (problem of problems.problems(); track problem.id) {
              <li>
                <button
                  type="button"
                  class="problem"
                  [attr.data-problem]="problem.key"
                  (click)="openProblem(problem.key)"
                >
                  <span class="problem__main">
                    <span class="problem__key">{{ problem.key }}</span>
                    <phx-badge [color]="severityColor(problem.severity)">{{
                      severityOf(problem.severity)
                    }}</phx-badge>
                  </span>
                  <span class="problem__meta">
                    <span class="problem__subject">{{ problem.subject }}</span>
                    <span class="problem__count">{{ seen(problem.count) }}</span>
                  </span>
                </button>
              </li>
            }
          </ul>
        }
      </section>
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    .centre {
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    .problems {
      padding: 0 20px 20px;
    }
    .problems__title {
      margin: 0 0 12px;
      font: var(--phx-heading-14);
      color: var(--phx-ink);
    }
    .problems__empty {
      margin: 0;
      font: var(--phx-text-14);
      color: var(--phx-ink-muted);
    }
    .problems__list {
      display: flex;
      flex-direction: column;
      gap: 8px;
      padding: 0;
      margin: 0;
      list-style: none;
    }
    .problem {
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
    .problem__main,
    .problem__meta {
      display: flex;
      gap: 8px;
      align-items: center;
      justify-content: space-between;
    }
    .problem__key {
      font: var(--phx-heading-14);
      overflow-wrap: anywhere;
    }
    .problem__meta {
      font: var(--phx-text-12-single);
      color: var(--phx-ink-muted);
    }
  `,
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelNotificationCentreComponent {
  protected readonly labels = inject(PHX_LABELS);
  protected readonly problems = inject(KeelProblems);
  private readonly notifications = inject(KeelNotifications);
  private readonly i18n = inject(KeelI18n);
  private readonly dates = inject(KeelDates);
  private readonly toasts = inject(PhxToastService);
  private readonly router = inject(Router);
  private readonly drawer = inject(PhxDrawerRef, { optional: true });
  protected readonly data = inject<NotificationCentreData>(PHX_DRAWER_DATA);

  readonly items = computed<NotificationView[]>(() =>
    this.notifications.messages().map((note) => this.viewOf(note)),
  );
  protected readonly title = computed(() => this.i18n.t(NOTIFICATIONS_TITLE_KEY));
  protected readonly unreadLabel = computed(() => this.i18n.t(NOTIFICATIONS_UNREAD_KEY));
  protected readonly dismissLabel = computed(() => this.i18n.t(NOTIFICATIONS_DISMISS_KEY));
  protected readonly clearLabel = computed(() => this.i18n.t(NOTIFICATIONS_CLEAR_KEY));
  protected readonly problemsTitle = computed(() => this.i18n.t(NOTIFICATIONS_PROBLEMS_KEY));
  protected readonly noProblems = computed(() => this.i18n.t(NOTIFICATIONS_NO_PROBLEMS_KEY));
  protected readonly locale = computed(() => this.dates.locale());

  async read(item: PhxNotification): Promise<void> {
    await this.post(this.notifications.mark([item.id]));
  }

  async readAll(): Promise<void> {
    const unread = this.notifications
      .messages()
      .filter((note) => note.unread)
      .map((note) => note.id);
    await this.post(this.notifications.mark(unread));
  }

  async dismiss(item: PhxNotification): Promise<void> {
    if (!item.read && !(await this.post(this.notifications.mark([item.id])))) {
      return;
    }
    this.notifications.dismiss(item.id);
  }

  async clear(): Promise<void> {
    await this.post(this.notifications.clear());
  }

  select(item: PhxNotification): void {
    const key = problemKeyOf(item.data as Note);
    if (key !== null) {
      this.openProblem(key);
    }
  }

  openProblem(key: string): void {
    this.drawer?.close();
    void this.router.navigate([`/${SCREEN_PATH}`, PROBLEM_LIST], {
      queryParams: { [SEARCH_PARAM]: key },
    });
  }

  protected severityOf(severity: Problem['severity']): string {
    return this.i18n.t(`${SEVERITY_PREFIX}.${severity}`);
  }

  protected severityColor(severity: Problem['severity']): PhxBadgeColor {
    return SEVERITY_COLORS[severity];
  }

  protected seen(count: number): string {
    return this.i18n.t(NOTIFICATIONS_SEEN_KEY, { count });
  }

  private viewOf(note: Note): NotificationView {
    return {
      id: note.id,
      status: LEVEL_STATUS[note.level],
      title: this.i18n.t(`${LEVEL_PREFIX}.${note.level}`),
      text: this.i18n.t(note.key, paramsOf(note)),
      date: Date.parse(note.createdAt),
      read: !note.unread,
      actions: [],
      data: note,
    };
  }

  private async post(pending: Promise<Envelope>): Promise<boolean> {
    const envelope = await pending.catch(() => null);
    if (envelope === null) {
      return false;
    }
    if (!envelope.ok) {
      this.toasts.danger(describeError(this.i18n, envelope.error));
    }
    return envelope.ok;
  }
}

function paramsOf(note: Note): Record<string, unknown> | undefined {
  const params = note.params;
  return typeof params === 'object' && params !== null && !Array.isArray(params)
    ? (params as Record<string, unknown>)
    : undefined;
}
