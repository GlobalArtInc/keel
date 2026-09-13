import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { KeelCallError, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { CallError } from '@globalart/keel/meta';
import {
  KeelNavigation,
  RETRY_KEY,
  SEARCH_PARAM,
  describeError,
  injectKeelViewContext,
} from '@globalart/keel/renderer';
import {
  PhxEmptyComponent,
  PhxErrorComponent,
  PhxLoaderComponent,
  PhxPageHeaderComponent,
  PhxPanelComponent,
} from '@globalart/phoenix-ui';
import { NOTE_LIST, TAG_EMPTY_KEY, TAG_LIST, loadTags, sized, type SizedTag } from './cloud';

export type CloudState =
  | { phase: 'loading' }
  | { phase: 'loaded'; tags: SizedTag[] }
  | { phase: 'failed'; error: CallError };

@Component({
  selector: 'demo-note-tag-cloud',
  imports: [
    PhxPageHeaderComponent,
    PhxPanelComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    PhxEmptyComponent,
  ],
  template: `
    <phx-page-header [title]="title()" />
    <phx-panel>
      @switch (state().phase) {
        @case ('loading') {
          <div class="loading"><phx-loader [size]="24" /></div>
        }
        @case ('failed') {
          <phx-error [text]="message()" [retryLabel]="retryLabel()" (retry)="load()" />
        }
        @case ('loaded') {
          @if (tags().length) {
            <div class="cloud">
              @for (tag of tags(); track tag.tag) {
                <button
                  type="button"
                  class="tag"
                  [style.font-size.px]="tag.size"
                  [attr.data-tag]="tag.tag"
                  [attr.data-count]="tag.count"
                  [attr.title]="tag.count"
                  (click)="open(tag.tag)"
                >
                  {{ tag.tag }}
                </button>
              }
            </div>
          } @else {
            <phx-empty icon="tag" [title]="emptyTitle()" [panel]="false" />
          }
        }
      }
    </phx-panel>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 16px;
      min-height: 0;
    }
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0;
    }
    .cloud {
      display: flex;
      flex-wrap: wrap;
      gap: 8px 16px;
      align-items: baseline;
      justify-content: center;
      padding: 8px 0;
    }
    .tag {
      padding: 2px 6px;
      font-family: inherit;
      line-height: 1.2;
      color: var(--phx-accent-text);
      background: transparent;
      border: 0;
      border-radius: var(--phx-radius-compact);
      cursor: pointer;
    }
    .tag:hover,
    .tag:focus-visible {
      background-color: var(--phx-tonal-hover);
      outline: none;
    }
    .tag:focus-visible {
      outline: var(--phx-focus-ring);
    }
  `,
  host: {
    '[attr.data-cloud]': 'func',
    '[attr.data-phase]': 'state().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class NoteTagCloudComponent {
  readonly context = injectKeelViewContext();
  readonly func = this.context.descriptor?.name ?? TAG_LIST;

  private readonly i18n = inject(KeelI18n);
  private readonly navigation = inject(KeelNavigation);
  private readonly held = signal<CloudState>({ phase: 'loading' });
  private sequence = 0;

  readonly state = this.held.asReadonly();

  readonly tags = computed(() => {
    const state = this.state();
    return state.phase === 'loaded' ? state.tags : [];
  });
  protected readonly title = computed(() =>
    this.i18n.t(this.context.descriptor?.ui?.title ?? `${TAG_LIST}.title`),
  );
  protected readonly emptyTitle = computed(() => this.i18n.t(TAG_EMPTY_KEY));
  protected readonly retryLabel = computed(() => this.i18n.t(RETRY_KEY));
  protected readonly message = computed(() => {
    const state = this.state();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  });

  constructor() {
    this.load();
  }

  load(): void {
    const started = ++this.sequence;
    this.held.set({ phase: 'loading' });
    loadTags(this.context.client, this.context.params, this.func).then(
      (rows) => {
        if (started === this.sequence) {
          this.held.set({ phase: 'loaded', tags: sized(rows) });
        }
      },
      (error: unknown) => {
        if (started === this.sequence) {
          this.held.set({
            phase: 'failed',
            error: error instanceof KeelCallError ? error.error : unreachable(error),
          });
        }
      },
    );
  }

  open(tag: string): void {
    void this.navigation.screen({ screen: NOTE_LIST, params: { [SEARCH_PARAM]: tag } });
  }
}
