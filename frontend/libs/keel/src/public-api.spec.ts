import { KEEL_VERSION } from '@globalart/keel';
import { version } from '../package.json';

describe('@globalart/keel', () => {
  it('carries the version of the package', () => {
    expect(KEEL_VERSION).toBe(version);
  });
});
