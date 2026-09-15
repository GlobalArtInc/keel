import { STORY_REQUEST_ID, errorEnvelope, okEnvelope, pageEnvelope } from '@globalart/keel/testing';

describe('the canned envelopes', () => {
  it('okEnvelope wraps data with a request id', () => {
    expect(okEnvelope({ id: 1 })).toEqual({
      ok: true,
      data: { id: 1 },
      meta: { requestId: STORY_REQUEST_ID },
    });
    const own = okEnvelope(null, 'req-7');
    expect(own.ok && own.meta).toEqual({ requestId: 'req-7' });
  });

  it('pageEnvelope counts the rows and carries the totals', () => {
    const rows = [{ id: 'a' }, { id: 'b' }];
    expect(pageEnvelope(rows)).toEqual({
      ok: true,
      data: { data: rows, total: 2, totals: null },
      meta: { requestId: STORY_REQUEST_ID },
    });
    const paged = pageEnvelope([], { records: 8 });
    expect(paged.ok && paged.data).toEqual({ data: [], total: 0, totals: { records: 8 } });
  });

  it('errorEnvelope keeps the fields of the error and its own request id', () => {
    const refused = errorEnvelope(
      {
        code: 'validation',
        key: 'keel.error.validation',
        fields: [{ path: 'title', key: 'note.error.exists' }],
      },
      'req-refused',
    );
    expect(refused).toEqual({
      ok: false,
      error: {
        requestId: 'req-refused',
        code: 'validation',
        key: 'keel.error.validation',
        fields: [{ path: 'title', key: 'note.error.exists' }],
      },
    });
    const own = errorEnvelope({ code: 'forbidden', key: 'k', requestId: 'req-own' });
    expect(!own.ok && own.error.requestId).toBe('req-own');
  });
});
