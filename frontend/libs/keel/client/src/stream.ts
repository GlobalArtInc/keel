import type { LogLevel, LogLine } from '@globalart/keel/meta';
import { Observable } from 'rxjs';
import { downloadUrl } from './download';
import type { EventSourceFactory } from './source';

export const LINE_EVENT = 'line';
export const END_EVENT = 'end';
const STREAM_PARAMS = 'q';

const LEVELS: readonly LogLevel[] = ['trace', 'debug', 'info', 'warn', 'error'];

export function streamUrl(func: string, params: unknown = {}): string {
  return `${downloadUrl(func)}?${STREAM_PARAMS}=${encodeURIComponent(JSON.stringify(params ?? {}))}`;
}

function isLogLine(value: unknown): value is LogLine {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const line = value as Partial<Record<keyof LogLine, unknown>>;
  return (
    typeof line.text === 'string' &&
    LEVELS.some((level) => level === line.level) &&
    (line.at === undefined || line.at === null || typeof line.at === 'string')
  );
}

export function openStream(
  open: EventSourceFactory,
  func: string,
  params: unknown = {},
): Observable<LogLine> {
  return new Observable<LogLine>((subscriber) => {
    const source = open(streamUrl(func, params), { withCredentials: true });
    source.addEventListener(LINE_EVENT, (event) => {
      const line = lineOf(event);
      if (line !== null) {
        subscriber.next(line);
      }
    });
    source.addEventListener(END_EVENT, () => {
      source.close();
      subscriber.complete();
    });
    source.addEventListener('error', () => {
      source.close();
      subscriber.error(new Error(`the stream ${func} closed`));
    });
    return () => source.close();
  });
}

function lineOf(event: Event): LogLine | null {
  const data: unknown = 'data' in event ? event.data : null;
  if (typeof data !== 'string') {
    return null;
  }
  try {
    const value: unknown = JSON.parse(data);
    return isLogLine(value) ? value : null;
  } catch {
    return null;
  }
}
