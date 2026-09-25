import { signal } from '@angular/core';
import {
  KeelCallError,
  KeelClient,
  requestIdOf,
  type CallOptions,
  type Catalogue,
  type DownloadResult,
  type Fetched,
  type UploadFiles,
} from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope, LogLine } from '@globalart/keel/meta';
import { Observable, Subject, of, throwError } from 'rxjs';
import { fixturesOf, readDump, type DumpLevel } from './dump';

export type CannedAnswer = Envelope | Envelope[];

export interface KeelMockFixtures {
  boot?: Boot;
  descriptors?: Descriptor[];
  catalogue?: Catalogue;
  answers?: Record<string, CannedAnswer>;
}

export interface RecordedCall {
  func: string;
  params: unknown;
  options: CallOptions;
}

export interface RecordedDownload {
  func: string;
  params: unknown;
}

export interface RecordedStream {
  func: string;
  params: unknown;
}

export interface RecordedUpload {
  func: string;
  params: unknown;
  files: UploadFiles;
  options: CallOptions;
}

export class KeelMockClient extends KeelClient {
  readonly calls: RecordedCall[] = [];
  readonly downloads: RecordedDownload[] = [];
  readonly uploads: RecordedUpload[] = [];
  readonly streams: RecordedStream[] = [];
  private readonly answers = new Map<string, Envelope[]>();
  private readonly files = new Map<string, DownloadResult>();
  private readonly feeds = new Map<string, Subject<LogLine>>();
  private readonly requestId = signal<string | null>(null);
  private readonly replay = signal(false);
  private readonly version = signal<string | null>(null);
  private readonly metaChanges = new Subject<string>();
  private readonly bursts = new Subject<void>();

  readonly lastRequestId = this.requestId.asReadonly();
  readonly replayed = this.replay.asReadonly();
  readonly metaVersion = this.version.asReadonly();
  readonly metaChanged = this.metaChanges.asObservable();
  readonly unauthenticated = this.bursts.asObservable();

  constructor(private readonly fixtures: KeelMockFixtures = {}) {
    super();
    for (const [func, canned] of Object.entries(fixtures.answers ?? {})) {
      this.answer(func, canned);
    }
    this.version.set(fixtures.boot?.meta ?? null);
  }

  static fromDump(dir: string, level: DumpLevel): KeelMockClient {
    return new KeelMockClient(fixturesOf(readDump(dir, level)));
  }

  answer(func: string, canned: CannedAnswer): this {
    const sequence = Array.isArray(canned) ? [...canned] : [canned];
    if (sequence.length === 0) {
      throw new RangeError(`the canned answers of ${func} are empty`);
    }
    this.answers.set(func, sequence);
    return this;
  }

  file(func: string, result: DownloadResult): this {
    this.files.set(func, result);
    return this;
  }

  feed(func: string): Subject<LogLine> {
    const opened = this.feeds.get(func);
    if (opened === undefined) {
      throw new Error(`the stream ${func} was not opened`);
    }
    return opened;
  }

  callsOf(func: string): RecordedCall[] {
    return this.calls.filter((call) => call.func === func);
  }

  changeMeta(version: string): void {
    this.version.set(version);
    this.metaChanges.next(version);
  }

  invalidateMeta(version: string | null): void {
    const next = version ?? this.version();
    if (next !== null) {
      this.changeMeta(next);
    }
  }

  expire(): void {
    this.bursts.next();
  }

  call<T = unknown>(
    func: string,
    params: unknown = {},
    options: CallOptions = {},
  ): Observable<Envelope<T>> {
    this.calls.push({ func, params, options });
    const sequence = this.answers.get(func);
    if (sequence === undefined) {
      throw new Error(`no canned answer for ${func}`);
    }
    const envelope = sequence.length > 1 ? sequence.shift() : sequence[0];
    if (envelope === undefined) {
      throw new Error(`no canned answer for ${func}`);
    }
    this.requestId.set(requestIdOf(envelope));
    return of(envelope as Envelope<T>);
  }

  download(func: string, params: unknown = {}): Observable<DownloadResult> {
    this.downloads.push({ func, params });
    const result = this.files.get(func);
    if (result === undefined) {
      throw new Error(`no canned file for ${func}`);
    }
    return of(result);
  }

  stream(func: string, params: unknown = {}): Observable<LogLine> {
    this.streams.push({ func, params });
    const opened = new Subject<LogLine>();
    this.feeds.set(func, opened);
    return opened.asObservable();
  }

  upload<T = unknown>(
    func: string,
    params: unknown,
    files: UploadFiles,
    options: CallOptions = {},
  ): Observable<Envelope<T>> {
    this.uploads.push({ func, params, files, options });
    return this.call<T>(func, params, options);
  }

  boot(): Observable<Fetched<Boot> | null> {
    const boot = this.fixtures.boot;
    if (boot === undefined) {
      throw new Error('no boot payload was handed to the mock client');
    }
    this.version.set(boot.meta);
    return of({ body: boot, etag: `"${boot.meta}"` });
  }

  descriptor(func: string): Observable<Descriptor> {
    const descriptor = (this.fixtures.descriptors ?? []).find((entry) => entry.name === func);
    if (descriptor === undefined) {
      return throwError(
        () =>
          new KeelCallError({
            code: 'not_found',
            key: 'keel.error.function_not_found',
            params: { function: func },
          }),
      );
    }
    return of(descriptor);
  }

  descriptors(): Observable<Descriptor[]> {
    return of([...(this.fixtures.descriptors ?? [])]);
  }

  catalogue(lang: string): Observable<Fetched<Catalogue> | null> {
    const catalogue = this.fixtures.catalogue ?? { lang, messages: {} };
    return of({ body: catalogue, etag: null });
  }
}
