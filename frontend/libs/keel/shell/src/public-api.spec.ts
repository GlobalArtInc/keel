import { KEEL_SHELL_VERSION } from '@globalart/keel/shell';
import { version } from '../../package.json';

describe('@globalart/keel/shell', () => {
  it('carries the version of the package', () => {
    expect(KEEL_SHELL_VERSION).toBe(version);
  });
});
