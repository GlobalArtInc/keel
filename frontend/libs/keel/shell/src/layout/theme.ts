import type { BootBrand } from '@globalart/keel/meta';
import type { PhxThemeService } from '@globalart/phoenix-ui';

export const BASE_CHROMA = 0.18;

export function chromaScale(chroma: number): number {
  return chroma / BASE_CHROMA;
}

export function applyBrand(theme: PhxThemeService, brand: BootBrand): void {
  theme.setHue(brand.hue);
  theme.setChroma(chromaScale(brand.chroma));
}
