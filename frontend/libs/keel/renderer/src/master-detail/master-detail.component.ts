import { NgTemplateOutlet } from '@angular/common';
import {
  ChangeDetectionStrategy,
  Component,
  TemplateRef,
  computed,
  inject,
  input,
  linkedSignal,
  output,
} from '@angular/core';
import { KeelI18n } from '@globalart/keel/client';
import type { Descriptor } from '@globalart/keel/meta';
import { PhxEmptyComponent, type PhxIconName } from '@globalart/phoenix-ui';
import { KeelListComponent } from '../list/list.component';
import { DETAIL_EMPTY_KEY } from '../messages';
import type { ScreenTarget } from '../navigation';
import { detailOf } from './selection';

export interface KeelDetailContext {
  $implicit: ScreenTarget;
}

const DETAIL_ICON: PhxIconName = 'list';

@Component({
  selector: 'keel-master-detail',
  imports: [NgTemplateOutlet, KeelListComponent, PhxEmptyComponent],
  template: `
    <keel-list
      class="master"
      [descriptor]="descriptor()"
      [params]="params()"
      picking
      (picked)="pick($event)"
    />
    <section class="detail" [attr.data-selected]="current()">
      @if (target(); as target) {
        <ng-container *ngTemplateOutlet="detail(); context: { $implicit: target }" />
      } @else {
        <phx-empty [title]="emptyTitle()" [icon]="icon" />
      }
    </section>
  `,
  styles: `
    :host {
      display: grid;
      grid-template-columns: minmax(0, 2fr) minmax(0, 3fr);
      gap: 24px;
      align-items: start;
      min-height: 0;
    }
    .master,
    .detail {
      min-width: 0;
    }
    @media (max-width: 960px) {
      :host {
        grid-template-columns: minmax(0, 1fr);
      }
    }
  `,
  host: {
    '[attr.data-master]': 'descriptor().name',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelMasterDetailComponent {
  readonly descriptor = input.required<Descriptor>();
  readonly params = input<Record<string, unknown>>({});
  readonly selected = input<string | null>(null);
  readonly detail = input.required<TemplateRef<KeelDetailContext>>();

  readonly selectedChange = output<string>();

  private readonly i18n = inject(KeelI18n);

  protected readonly icon = DETAIL_ICON;
  readonly current = linkedSignal(() => this.selected());
  readonly target = computed(() => detailOf(this.descriptor(), this.current()));
  protected readonly emptyTitle = computed(() => this.i18n.t(DETAIL_EMPTY_KEY));

  pick(id: string): void {
    this.current.set(id);
    this.selectedChange.emit(id);
  }
}
