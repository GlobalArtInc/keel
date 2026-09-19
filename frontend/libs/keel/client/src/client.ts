import {
  HttpClient,
  HttpErrorResponse,
  HttpHeaders,
  HttpParams,
  type HttpResponse,
} from '@angular/common/http';
import { Injectable, inject, signal, type Signal } from '@angular/core';
import type { Boot, CallError, Descriptor, Envelope, Messages } from '@globalart/keel/meta';
import { PhxGlobalProgressService } from '@globalart/phoenix-ui';
import {
  Observable,
  Subject,
  catchError,
  from,
  map,
  of,
  retry,
  switchMap,
  tap,
  throwError,
} from 'rxjs';
import { CONTENT_DISPOSITION, downloadUrl, filenameOf, type DownloadResult } from './download';
import { tracked } from './progress';

export const API_PREFIX = '/api/v1';
export const REQUEST_ID_HEADER = 'X-Keel-Request-Id';
export const META_VERSION_HEADER = 'X-Keel-Meta-Version';
export const IDEMPOTENT_REPLAY_HEADER = 'X-Keel-Idempotent-Replay';
export const IDEMPOTENCY_KEY_HEADER = 'Idempotency-Key';
export const LANG_PARAM = 'lang';
export const UNAUTHENTICATED_KEY = 'keel.error.unauthenticated';

const IF_NONE_MATCH = 'If-None-Match';
const ETAG = 'ETag';
const AUTHORIZATION = 'Authorization';
const UNAUTHENTICATED = 401;
const NOT_MODIFIED = 304;

export interface CallOptions {
  idempotencyKey?: string;
  requestId?: string;
  bearer?: string;
}

export interface Fetched<T> {
  body: T;
  etag: string | null;
}

export interface Catalogue {
  lang: string;
  messages: Messages;
}

export class KeelCallError extends Error {
  constructor(readonly error: CallError) {
    super(`${error.code}: ${error.key}`);
    this.name = 'KeelCallError';
  }
}

export abstract class KeelClient {
  abstract readonly lastRequestId: Signal<string | null>;
  abstract readonly replayed: Signal<boolean>;
  abstract readonly metaVersion: Signal<string | null>;
  abstract readonly metaChanged: Observable<string>;
  abstract readonly unauthenticated: Observable<void>;

  abstract invalidateMeta(version: string | null): void;

  abstract call<T = unknown>(
    func: string,
    params?: unknown,
    options?: CallOptions,
  ): Observable<Envelope<T>>;

  abstract download(func: string, params?: unknown): Observable<DownloadResult>;

  abstract boot(etag?: string | null): Observable<Fetched<Boot> | null>;

  abstract descriptor(func: string): Observable<Descriptor>;

  abstract descriptors(): Observable<Descriptor[]>;

  abstract catalogue(lang: string, etag?: string | null): Observable<Fetched<Catalogue> | null>;
}

export function isEnvelope(value: unknown): value is Envelope {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as { ok?: unknown }).ok === 'boolean'
  );
}

export function requestIdOf(envelope: Envelope): string | null {
  return (envelope.ok ? envelope.meta.requestId : envelope.error.requestId) ?? null;
}

export function callHeaders(options: CallOptions): HttpHeaders {
  let headers = new HttpHeaders();
  if (options.idempotencyKey !== undefined) {
    headers = headers.set(IDEMPOTENCY_KEY_HEADER, options.idempotencyKey);
  }
  if (options.requestId !== undefined) {
    headers = headers.set(REQUEST_ID_HEADER, options.requestId);
  }
  if (options.bearer !== undefined) {
    headers = headers.set(AUTHORIZATION, `Bearer ${options.bearer}`);
  }
  return headers;
}

export function sessionExpired(body: unknown): boolean {
  return !isEnvelope(body) || body.ok || body.error.key === UNAUTHENTICATED_KEY;
}

function isNetworkFailure(error: unknown): boolean {
  return error instanceof HttpErrorResponse && error.status === 0;
}

function parsed(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function bodyOf<T>(response: HttpResponse<T>): T {
  if (response.body === null) {
    throw new TypeError(`${response.url ?? 'the request'} answered no body`);
  }
  return response.body;
}

@Injectable({ providedIn: 'root' })
export class KeelHttpClient extends KeelClient {
  private readonly http = inject(HttpClient);
  private readonly progress = inject(PhxGlobalProgressService);
  private readonly requestId = signal<string | null>(null);
  private readonly replay = signal(false);
  private readonly version = signal<string | null>(null);
  private readonly metaChanges = new Subject<string>();
  private readonly bursts = new Subject<void>();
  private armed = true;

  readonly lastRequestId = this.requestId.asReadonly();
  readonly replayed = this.replay.asReadonly();
  readonly metaVersion = this.version.asReadonly();
  readonly metaChanged = this.metaChanges.asObservable();
  readonly unauthenticated = this.bursts.asObservable();

  invalidateMeta(version: string | null): void {
    const next = version ?? this.version();
    if (next === null) {
      return;
    }
    this.version.set(next);
    this.metaChanges.next(next);
  }

  call<T = unknown>(
    func: string,
    params: unknown = {},
    options: CallOptions = {},
  ): Observable<Envelope<T>> {
    const url = `${API_PREFIX}/call/${encodeURIComponent(func)}`;
    return this.http
      .post<Envelope<T>>(url, params ?? {}, { headers: callHeaders(options), observe: 'response' })
      .pipe(
        retry({
          count: 1,
          delay: (error: unknown) =>
            options.idempotencyKey !== undefined && isNetworkFailure(error)
              ? of(0)
              : throwError(() => error),
        }),
        map((response) =>
          this.settle<T>(response.headers, response.status, bodyOf(response), func),
        ),
        catchError((error: unknown) => {
          if (error instanceof HttpErrorResponse && isEnvelope(error.error)) {
            return of(this.settle<T>(error.headers, error.status, error.error, func));
          }
          return throwError(() => error);
        }),
        tracked(this.progress),
      );
  }

  download(func: string, params: unknown = {}): Observable<DownloadResult> {
    return this.http
      .post(downloadUrl(func), params ?? {}, { observe: 'response', responseType: 'blob' })
      .pipe(
        map((response): DownloadResult => {
          this.observe(response.headers, response.status);
          return {
            ok: true,
            file: {
              blob: bodyOf(response),
              filename: filenameOf(response.headers.get(CONTENT_DISPOSITION)),
            },
          };
        }),
        catchError((error: unknown) => {
          if (!(error instanceof HttpErrorResponse) || !(error.error instanceof Blob)) {
            return throwError(() => error);
          }
          return from(error.error.text()).pipe(
            switchMap((text) => {
              const body = parsed(text);
              this.observe(error.headers, error.status, body);
              return isEnvelope(body) && !body.ok
                ? of<DownloadResult>({ ok: false, error: body.error })
                : throwError(() => error);
            }),
          );
        }),
        tracked(this.progress),
      );
  }

  boot(etag: string | null = null): Observable<Fetched<Boot> | null> {
    return this.get<Boot>(`${API_PREFIX}/boot`, etag).pipe(
      tap((fetched) => {
        if (fetched !== null) {
          this.version.set(fetched.body.meta);
        }
      }),
    );
  }

  descriptor(func: string): Observable<Descriptor> {
    return this.body<Descriptor>(`${API_PREFIX}/functions/${encodeURIComponent(func)}`);
  }

  descriptors(): Observable<Descriptor[]> {
    return this.body<Descriptor[]>(`${API_PREFIX}/functions`);
  }

  catalogue(lang: string, etag: string | null = null): Observable<Fetched<Catalogue> | null> {
    const params = new HttpParams().set(LANG_PARAM, lang);
    return this.get<Envelope<Catalogue>>(`${API_PREFIX}/i18n`, etag, params).pipe(
      map((fetched) => {
        if (fetched === null) {
          return null;
        }
        if (!fetched.body.ok) {
          throw new KeelCallError(fetched.body.error);
        }
        return { body: fetched.body.data, etag: fetched.etag };
      }),
    );
  }

  private body<T>(url: string): Observable<T> {
    return this.get<T>(url, null).pipe(
      map((fetched) => {
        if (fetched === null) {
          throw new TypeError(`${url} answered not modified without a version to match`);
        }
        return fetched.body;
      }),
    );
  }

  private get<T>(
    url: string,
    etag: string | null,
    params?: HttpParams,
  ): Observable<Fetched<T> | null> {
    let headers = new HttpHeaders();
    if (etag !== null) {
      headers = headers.set(IF_NONE_MATCH, etag);
    }
    return this.http.get<T>(url, { headers, params, observe: 'response' }).pipe(
      map((response) => {
        this.observe(response.headers, response.status);
        return { body: bodyOf(response), etag: response.headers.get(ETAG) };
      }),
      catchError((error: unknown) => {
        if (error instanceof HttpErrorResponse) {
          this.observe(error.headers, error.status, error.error);
          if (error.status === NOT_MODIFIED) {
            return of(null);
          }
          const body: unknown = error.error;
          if (isEnvelope(body) && !body.ok) {
            return throwError(() => new KeelCallError(body.error));
          }
        }
        return throwError(() => error);
      }),
    );
  }

  private settle<T>(
    headers: HttpHeaders,
    status: number,
    body: unknown,
    func: string,
  ): Envelope<T> {
    this.observe(headers, status, body);
    if (!isEnvelope(body)) {
      throw new TypeError(`${func} answered something other than a call envelope`);
    }
    const envelope = body as Envelope<T>;
    if (headers.get(REQUEST_ID_HEADER) === null) {
      this.requestId.set(requestIdOf(envelope));
    }
    this.replay.set(headers.has(IDEMPOTENT_REPLAY_HEADER));
    this.watch(headers.get(META_VERSION_HEADER));
    return envelope;
  }

  private observe(headers: HttpHeaders, status: number, body?: unknown): void {
    const requestId = headers.get(REQUEST_ID_HEADER);
    if (requestId !== null) {
      this.requestId.set(requestId);
    }
    if (status === UNAUTHENTICATED) {
      if (this.armed && sessionExpired(body)) {
        this.armed = false;
        this.bursts.next();
      }
    } else if (status >= 200 && status < 300) {
      this.armed = true;
    }
  }

  private watch(version: string | null): void {
    if (version === null) {
      return;
    }
    const known = this.version();
    if (known === version) {
      return;
    }
    this.version.set(version);
    if (known !== null) {
      this.metaChanges.next(version);
    }
  }
}
