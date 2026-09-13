import { KEEL_FORMS_VERSION } from '@globalart/keel/forms';
import { version } from '../../package.json';

describe('@globalart/keel/forms', () => {
  it('carries the version of the package', () => {
    expect(KEEL_FORMS_VERSION).toBe(version);
  });
});
