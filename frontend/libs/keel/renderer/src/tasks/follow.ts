import { Injectable, Injector, inject } from '@angular/core';
import {
  KeelClient,
  KeelEvents,
  KeelI18n,
  TASK_CANCEL,
  TASK_GET,
  isActive,
  isDownloadRef,
  type TaskDetail,
} from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type {
  CallError,
  Envelope,
  Id,
  TaskProgress,
  TaskRef,
  TaskState,
} from '@globalart/keel/meta';
import {
  PHX_LABELS,
  PHX_PROGRESS_MODAL,
  PhxModalService,
  PhxProgressModalController,
  PhxToastService,
  type PhxModalRef,
} from '@globalart/phoenix-ui';
import { Subject, Subscription, filter, firstValueFrom, type Observable } from 'rxjs';
import { describeError } from '../messages';
import { KeelDownloads } from './download';
import { KEEL_FOLLOWED_TASK, KeelProgressComponent, type FollowedTask } from './progress.component';

export const TASK_POLL_MS = 5000;
export const TASK_FAILED_KEY = 'keel.shell.tasks.failed';

export interface TaskOpener {
  title: string;
  cancelable?: boolean;
}

export type Frame = Pick<TaskProgress, 'state' | 'done' | 'total' | 'message'>;

export function valueOf(frame: Frame): number | null {
  const total = frame.total ?? null;
  if (total === null || total <= 0) {
    return null;
  }
  return Math.min(100, Math.round((frame.done / total) * 100));
}

export function failureOf(detail: TaskDetail): CallError {
  return (
    detail.error ?? {
      code: 'internal',
      key: TASK_FAILED_KEY,
      params: { function: detail.function },
      requestId: detail.requestId ?? null,
    }
  );
}

@Injectable({ providedIn: 'root' })
export class KeelTasks {
  private readonly client = inject(KeelClient);
  private readonly events = inject(KeelEvents);
  private readonly i18n = inject(KeelI18n);
  private readonly labels = inject(PHX_LABELS);
  private readonly modals = inject(PhxModalService);
  private readonly toasts = inject(PhxToastService);
  private readonly downloads = inject(KeelDownloads);
  private readonly injector = inject(Injector);
  private readonly wanted = new Subject<Id>();

  readonly details$: Observable<Id> = this.wanted.asObservable();

  follow<T = unknown>(ref: TaskRef, opener: TaskOpener): Promise<Envelope<T> | null> {
    const id = ref.task.id;
    return new Promise((resolve) => {
      let modal: PhxModalRef | null = null;
      let settled = false;
      let detached = false;
      let timer: ReturnType<typeof setTimeout> | null = null;
      const controller = new PhxProgressModalController(
        {
          title: opener.title,
          cancelable: opener.cancelable ?? true,
          cancelLabel: this.labels.cancel,
          value: null,
        },
        () => modal,
      );
      const detach = (): void => {
        detached = true;
        modal?.close();
        this.wanted.next(id);
      };
      const followed: FollowedTask = {
        id,
        details: this.wanted.observed ? detach : null,
      };
      const subscriptions = new Subscription();
      const stop = (): void => {
        settled = true;
        subscriptions.unsubscribe();
        if (timer !== null) {
          clearTimeout(timer);
          timer = null;
        }
      };
      const settle = (outcome: Envelope<T> | null): void => {
        stop();
        controller.close();
        resolve(outcome);
      };
      const finish = async (state: TaskState): Promise<void> => {
        stop();
        const detail = await this.detail(id);
        if (!detail.ok) {
          this.fail(opener, detail.error);
          settle(detail);
          return;
        }
        if (state === 'done') {
          const result = detail.data.result ?? null;
          controller.close();
          if (isDownloadRef(result)) {
            await this.downloads.save(result.download, opener.title);
          }
          resolve({
            ok: true,
            data: result as T,
            meta: { requestId: detail.data.requestId ?? '' },
          });
          return;
        }
        if (state === 'failed') {
          const error = failureOf(detail.data);
          this.fail(opener, error);
          settle({ ok: false, error });
          return;
        }
        settle(null);
      };
      const apply = (frame: Frame): void => {
        if (settled) {
          return;
        }
        controller.update({
          value: valueOf(frame),
          status: frame.message ? this.i18n.t(frame.message) : null,
        });
        if (!isActive(frame.state)) {
          void finish(frame.state);
        }
      };
      const arm = (): void => {
        if (timer !== null) {
          clearTimeout(timer);
        }
        timer = setTimeout(() => void poll(), TASK_POLL_MS);
      };
      const poll = async (): Promise<void> => {
        if (settled) {
          return;
        }
        const detail = await this.detail(id);
        if (settled) {
          return;
        }
        if (!detail.ok) {
          this.fail(opener, detail.error);
          settle(detail);
          return;
        }
        arm();
        apply(detail.data);
      };
      subscriptions.add(
        this.events.taskProgress$.pipe(filter((frame) => frame.id === id)).subscribe((frame) => {
          arm();
          apply(frame);
        }),
      );
      subscriptions.add(
        controller.cancelled.subscribe(() => {
          if (settled) {
            return;
          }
          stop();
          void this.cancel(id, opener).then(() => resolve(null));
        }),
      );
      const injector = Injector.create({
        parent: this.injector,
        providers: [
          { provide: PHX_PROGRESS_MODAL, useValue: controller },
          { provide: KEEL_FOLLOWED_TASK, useValue: followed },
        ],
      });
      modal = this.modals.open(KeelProgressComponent, {
        width: 400,
        closable: false,
        closeOnEscape: false,
        closeOnBackdrop: false,
        label: opener.title,
        injector,
      });
      subscriptions.add(
        modal.afterClosed.subscribe(() => {
          if (!settled && !detached) {
            stop();
            resolve(null);
          }
        }),
      );
      arm();
    });
  }

  private async detail(id: Id): Promise<Envelope<TaskDetail>> {
    return firstValueFrom(this.client.call<TaskDetail>(TASK_GET, { id })).catch(
      (error: unknown): Envelope<TaskDetail> => ({ ok: false, error: unreachable(error) }),
    );
  }

  private async cancel(id: Id, opener: TaskOpener): Promise<void> {
    const envelope = await firstValueFrom(this.client.call(TASK_CANCEL, { id })).catch(
      (error: unknown): Envelope => ({ ok: false, error: unreachable(error) }),
    );
    if (!envelope.ok) {
      this.fail(opener, envelope.error);
    }
  }

  private fail(opener: TaskOpener, error: CallError): void {
    this.toasts.danger(describeError(this.i18n, error), { title: opener.title });
  }
}
