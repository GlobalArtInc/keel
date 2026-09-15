import type { Boot } from '@globalart/keel/meta';
import { BOOT_NONE, BOOT_ONE, BOOT_TWO } from './generated/boot';

export interface KeelBootFixtures {
  none: Boot;
  one: Boot;
  two: Boot;
}

export const KEEL_BOOT: Readonly<KeelBootFixtures> = {
  none: BOOT_NONE,
  one: BOOT_ONE,
  two: BOOT_TWO,
};
