import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { KeelBoot, KeelI18n } from '@globalart/keel/client';
import type { BootProduct } from '@globalart/keel/meta';
import {
  PhxDrawerBodyComponent,
  PhxDrawerHeaderComponent,
  PhxPropListComponent,
  type PhxProp,
} from '@globalart/phoenix-ui';

export const ABOUT_TITLE_KEY = 'keel.shell.about.title';
export const ABOUT_VERSION_KEY = 'keel.shell.about.version';
export const ABOUT_API_KEY = 'keel.shell.about.api';
export const ABOUT_PRODUCTS_KEY = 'keel.shell.about.products';
export const ABOUT_NO_PRODUCTS_KEY = 'keel.shell.about.no_products';
export const ABOUT_EDITION_KEY = 'keel.shell.about.edition';

export interface ProductView {
  id: string;
  name: string;
  edition: string | null;
}

export function productViews(
  products: readonly BootProduct[],
  label: (key: string) => string,
): ProductView[] {
  return products.map((product) => ({
    id: product.id,
    name: label(product.name),
    edition: product.edition ?? null,
  }));
}

@Component({
  selector: 'keel-about',
  imports: [PhxDrawerHeaderComponent, PhxDrawerBodyComponent, PhxPropListComponent],
  template: `
    <phx-drawer-header [title]="title()" />
    <phx-drawer-body>
      <phx-prop-list class="platform" [items]="platform()" />
      <h3 class="heading">{{ productsTitle() }}</h3>
      @if (products().length === 0) {
        <p class="none">{{ noProducts() }}</p>
      } @else {
        <ul class="products">
          @for (product of products(); track product.id) {
            <li class="product" [attr.data-product]="product.id">
              <span class="product__name">{{ product.name }}</span>
              @if (product.edition !== null) {
                <span class="product__edition">{{ editionOf(product.edition) }}</span>
              }
            </li>
          }
        </ul>
      }
    </phx-drawer-body>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    .platform {
      margin-bottom: 20px;
    }
    .heading {
      margin: 0 0 12px;
      font: var(--phx-heading-14);
      color: var(--phx-ink);
    }
    .none {
      margin: 0;
      font: var(--phx-text-14);
      color: var(--phx-ink-muted);
    }
    .products {
      display: flex;
      flex-direction: column;
      gap: 8px;
      padding: 0;
      margin: 0;
      list-style: none;
    }
    .product {
      display: flex;
      gap: 8px;
      align-items: baseline;
      justify-content: space-between;
      padding: 12px;
      background-color: var(--phx-tonal);
      border-radius: var(--phx-radius-medium);
    }
    .product__name {
      font: var(--phx-heading-14);
    }
    .product__edition {
      font: var(--phx-text-12-single);
      color: var(--phx-ink-muted);
    }
  `,
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelAboutComponent {
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);

  readonly products = computed(() => productViews(this.boot.products(), (key) => this.i18n.t(key)));
  readonly platform = computed<PhxProp[]>(() => {
    const keel = this.boot.payload()?.keel ?? null;
    return [
      { label: this.i18n.t(ABOUT_VERSION_KEY), value: keel?.version ?? '', mono: true },
      { label: this.i18n.t(ABOUT_API_KEY), value: keel?.api ?? '', mono: true },
    ];
  });

  protected readonly title = computed(() => this.i18n.t(ABOUT_TITLE_KEY));
  protected readonly productsTitle = computed(() => this.i18n.t(ABOUT_PRODUCTS_KEY));
  protected readonly noProducts = computed(() => this.i18n.t(ABOUT_NO_PRODUCTS_KEY));

  protected editionOf(edition: string): string {
    return this.i18n.t(ABOUT_EDITION_KEY, { edition });
  }
}
