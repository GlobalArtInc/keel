import { InjectionToken } from '@angular/core';

export interface EventSourceLike {
  readonly readyState: number;
  addEventListener(type: string, listener: (event: Event) => void): void;
  close(): void;
}

export type EventSourceFactory = (url: string, init: EventSourceInit) => EventSourceLike;

export const KEEL_EVENT_SOURCE = new InjectionToken<EventSourceFactory>('KEEL_EVENT_SOURCE', {
  providedIn: 'root',
  factory: () => (url, init) => new EventSource(url, init),
});
