import type { LogLine } from '@globalart/keel/meta';
import type { EventSourceLike } from './source';
import { END_EVENT, LINE_EVENT, openStream, streamUrl } from './stream';

class FakeSource implements EventSourceLike {
  readyState = 0;
  closed = false;
  private readonly listeners = new Map<string, ((event: Event) => void)[]>();

  constructor(readonly url: string) {}

  addEventListener(type: string, listener: (event: Event) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  close(): void {
    this.closed = true;
  }

  emit(type: string, data?: string): void {
    const event = data === undefined ? new Event(type) : new MessageEvent(type, { data });
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
  }
}

describe('openStream', () => {
  it('opens the raw route with the parameters and yields lines until the end event', () => {
    const sources: FakeSource[] = [];
    const lines: LogLine[] = [];
    let completed = false;
    openStream(
      (url) => {
        const source = new FakeSource(url);
        sources.push(source);
        return source;
      },
      'dns.zone.log',
      { id: 'z1' },
    ).subscribe({ next: (line) => lines.push(line), complete: () => (completed = true) });

    const source = sources[0];
    expect(source.url).toBe(streamUrl('dns.zone.log', { id: 'z1' }));
    expect(source.url).toBe(`/api/v1/x/dns/zone.log?q=${encodeURIComponent('{"id":"z1"}')}`);
    source.emit(LINE_EVENT, JSON.stringify({ level: 'info', text: 'applied' }));
    source.emit(LINE_EVENT, 'not json');
    source.emit(LINE_EVENT, JSON.stringify({ level: 'loud', text: 'refused' }));
    source.emit(END_EVENT, JSON.stringify({ reason: 'done' }));
    expect(lines).toEqual([{ level: 'info', text: 'applied' }]);
    expect(completed).toBe(true);
    expect(source.closed).toBe(true);
  });

  it('closes the source and fails when the connection breaks', () => {
    const source = new FakeSource('');
    let failed: unknown = null;
    openStream(() => source, 'keel.logs.stream').subscribe({
      error: (error: unknown) => (failed = error),
    });
    source.emit('error');
    expect(failed).toBeInstanceOf(Error);
    expect(source.closed).toBe(true);
  });
});
