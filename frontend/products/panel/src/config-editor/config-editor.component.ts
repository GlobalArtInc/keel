import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { KeelCallError, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { CallError, Id } from '@globalart/keel/meta';
import { RETRY_KEY, describeError, injectKeelViewContext } from '@globalart/keel/renderer';
import {
  PhxButtonComponent,
  PhxEmptyComponent,
  PhxErrorComponent,
  PhxLoaderComponent,
  PhxPageHeaderComponent,
  PhxPanelComponent,
  PhxSelectComponent,
  PhxToastService,
  type PhxSelectOption,
} from '@globalart/phoenix-ui';
import { PhxCodeEditorComponent, type PhxCodeLanguage } from '@globalart/phoenix-ui/code';
import {
  ConfigApi,
  KEYS,
  NO_ERRORS,
  PARTS,
  partErrorsOf,
  type ConfigPart,
  type PartErrors,
  type SiteChoice,
  type SiteConfig,
} from './config';

export type SitesState =
  | { phase: 'loading' }
  | { phase: 'failed'; error: CallError }
  | { phase: 'loaded'; sites: SiteChoice[] };

export type ConfigState =
  | { phase: 'idle' }
  | { phase: 'loading' }
  | { phase: 'failed'; error: CallError }
  | { phase: 'loaded'; config: SiteConfig };

const EDITOR_HEIGHT = 360;
const LANGUAGES: Record<ConfigPart, PhxCodeLanguage> = { server: 'nginx', pool: 'ini' };

function callErrorOf(error: unknown): CallError {
  return error instanceof KeelCallError ? error.error : unreachable(error);
}

function paramOf(params: Record<string, unknown>, name: string): string | null {
  const value = params[name];
  return typeof value === 'string' && value !== '' ? value : null;
}

function texts(): Record<ConfigPart, FormControl<string>> {
  return {
    server: new FormControl('', { nonNullable: true }),
    pool: new FormControl('', { nonNullable: true }),
  };
}

@Component({
  selector: 'panel-config-editor',
  imports: [
    ReactiveFormsModule,
    PhxButtonComponent,
    PhxCodeEditorComponent,
    PhxEmptyComponent,
    PhxErrorComponent,
    PhxLoaderComponent,
    PhxPageHeaderComponent,
    PhxPanelComponent,
    PhxSelectComponent,
  ],
  template: `
    <phx-page-header [title]="t(keys.title)" />
    <phx-panel>
      @switch (sites().phase) {
        @case ('loading') {
          <div class="loading"><phx-loader [size]="24" /></div>
        }
        @case ('failed') {
          <phx-error [text]="sitesMessage()" [retryLabel]="t(retry)" (retry)="loadSites()" />
        }
        @case ('loaded') {
          @if (choices().length === 0) {
            <phx-empty icon="globe" [title]="t(keys.none)" [panel]="false" />
          } @else {
            <div class="bar">
              <phx-select
                class="site"
                [label]="t(keys.site)"
                [options]="choices()"
                [value]="site()"
                (change)="pick($event)"
              />
              @if (loaded() !== null) {
                <button
                  phxButton="primary"
                  type="button"
                  icon="save"
                  class="save"
                  [loading]="busy()"
                  (click)="save()"
                >
                  {{ t(keys.save) }}
                </button>
              }
            </div>
            @switch (config().phase) {
              @case ('idle') {
                <phx-empty icon="file-code" [title]="t(keys.pick)" [panel]="false" />
              }
              @case ('loading') {
                <div class="loading"><phx-loader [size]="24" /></div>
              }
              @case ('failed') {
                <phx-error [text]="configMessage()" [retryLabel]="t(retry)" (retry)="load()" />
              }
              @case ('loaded') {
                @for (part of parts; track part) {
                  <section class="part" [attr.data-part]="part">
                    <h3 class="heading">{{ t(keys[part]) }}</h3>
                    <div class="pair">
                      @defer (on immediate) {
                        <phx-code-editor
                          class="override"
                          [language]="languages[part]"
                          [label]="t(keys.overrides)"
                          [hint]="hints()[part]"
                          [error]="errors()[part]"
                          [invalid]="errors()[part] !== null"
                          [height]="height"
                          [formControl]="overrides[part]"
                        />
                        <phx-code-editor
                          class="rendered"
                          readonly
                          [language]="languages[part]"
                          [label]="t(keys.rendered)"
                          [placeholder]="missing()[part]"
                          [height]="height"
                          [formControl]="rendered[part]"
                        />
                      }
                    </div>
                  </section>
                }
              }
            }
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
    .bar {
      display: flex;
      flex-wrap: wrap;
      gap: 12px;
      align-items: flex-end;
      justify-content: space-between;
      margin-bottom: 12px;
    }
    .site {
      min-width: 240px;
    }
    .part + .part {
      margin-top: 24px;
    }
    .heading {
      margin: 0 0 8px;
      font-size: 14px;
      font-weight: 600;
      color: var(--phx-text);
    }
    .pair {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 16px;
      align-items: start;
    }
    @media (max-width: 960px) {
      .pair {
        grid-template-columns: minmax(0, 1fr);
      }
    }
  `,
  host: {
    'data-view': 'hosting.site.config-editor',
    '[attr.data-phase]': 'config().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ConfigEditorComponent {
  readonly context = injectKeelViewContext();
  readonly keys = KEYS;
  readonly retry = RETRY_KEY;
  readonly parts = PARTS;
  readonly languages = LANGUAGES;
  readonly height = EDITOR_HEIGHT;
  readonly overrides = texts();
  readonly rendered = texts();

  private readonly i18n = inject(KeelI18n);
  private readonly toasts = inject(PhxToastService);
  private readonly api = new ConfigApi(this.context.client);
  private readonly heldSites = signal<SitesState>({ phase: 'loading' });
  private readonly heldConfig = signal<ConfigState>({ phase: 'idle' });
  private readonly heldSite = signal<Id | null>(paramOf(this.context.params, 'id'));
  private readonly heldErrors = signal<PartErrors>(NO_ERRORS);
  private readonly working = signal(false);
  private sequence = 0;

  readonly sites = this.heldSites.asReadonly();
  readonly config = this.heldConfig.asReadonly();
  readonly site = this.heldSite.asReadonly();
  readonly busy = this.working.asReadonly();

  readonly choices = computed<PhxSelectOption<Id>[]>(() => {
    const state = this.sites();
    return state.phase === 'loaded' ? state.sites : [];
  });
  readonly loaded = computed(() => {
    const state = this.config();
    return state.phase === 'loaded' ? state.config : null;
  });
  readonly hints = computed<Record<ConfigPart, string | null>>(() => {
    const domain = this.loaded()?.domain ?? '';
    return {
      server: this.t(KEYS.serverHint, { domain }),
      pool: this.t(KEYS.poolHint, { domain }),
    };
  });
  readonly missing = computed<Record<ConfigPart, string | null>>(() => {
    const config = this.loaded();
    const missing = (part: ConfigPart) =>
      config !== null && config.rendered[part] === null ? this.t(KEYS.notRendered) : null;
    return {
      server: missing('server'),
      pool: config !== null && config.php === null ? this.t(KEYS.noPool) : missing('pool'),
    };
  });
  readonly errors = computed<Record<ConfigPart, string | null>>(() => {
    const errors = this.heldErrors();
    const text = (part: ConfigPart) => {
      const field = errors[part];
      return field === null ? null : this.t(field.key ?? field.code ?? '', field.params);
    };
    return { server: text('server'), pool: text('pool') };
  });
  readonly sitesMessage = computed(() => {
    const state = this.sites();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  });
  readonly configMessage = computed(() => {
    const state = this.config();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  });

  constructor() {
    this.loadSites();
    if (this.site() !== null) {
      this.load();
    }
  }

  t(key: string, params?: Record<string, unknown>): string {
    return this.i18n.t(key, params);
  }

  loadSites(): void {
    this.heldSites.set({ phase: 'loading' });
    this.api.sites().then(
      (sites) => this.heldSites.set({ phase: 'loaded', sites }),
      (error: unknown) => this.heldSites.set({ phase: 'failed', error: callErrorOf(error) }),
    );
  }

  load(): void {
    const site = this.site();
    if (site === null) {
      this.heldConfig.set({ phase: 'idle' });
      return;
    }
    this.heldConfig.set({ phase: 'loading' });
    this.fetch(site);
  }

  pick(value: Id | Id[] | null): void {
    this.heldSite.set(typeof value === 'string' ? value : null);
    this.heldErrors.set(NO_ERRORS);
    this.load();
  }

  private fetch(site: Id): void {
    const started = ++this.sequence;
    this.api.get(site).then(
      (config) => {
        if (started === this.sequence) {
          this.show(config);
        }
      },
      (error: unknown) => {
        if (started === this.sequence) {
          this.heldConfig.set({ phase: 'failed', error: callErrorOf(error) });
        }
      },
    );
  }

  async save(): Promise<void> {
    const config = this.loaded();
    if (config === null || this.working()) {
      return;
    }
    this.working.set(true);
    this.heldErrors.set(NO_ERRORS);
    try {
      await this.api.set(config.id, {
        server: this.overrides.server.value,
        pool: this.overrides.pool.value,
      });
      this.toasts.success(this.t(KEYS.saved, { domain: config.domain }));
      this.fetch(config.id);
    } catch (error: unknown) {
      const failure = callErrorOf(error);
      const errors = partErrorsOf(failure);
      if (errors === null) {
        this.toasts.danger(describeError(this.i18n, failure), { title: this.t(KEYS.title) });
      } else {
        this.heldErrors.set(errors);
      }
    } finally {
      this.working.set(false);
    }
  }

  private show(config: SiteConfig): void {
    for (const part of PARTS) {
      this.overrides[part].setValue(config[part]);
      this.rendered[part].setValue(config.rendered[part] ?? '');
    }
    this.heldConfig.set({ phase: 'loaded', config });
  }
}
