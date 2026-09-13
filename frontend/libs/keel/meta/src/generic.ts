import type { Envelope as EnvelopeOfUnknown, Page as PageOfUnknown } from './generated';

type OkEnvelope = Extract<EnvelopeOfUnknown, { ok: true }>;
type ErrorEnvelope = Extract<EnvelopeOfUnknown, { ok: false }>;

export type Envelope<T = unknown> = (Omit<OkEnvelope, 'data'> & { data: T }) | ErrorEnvelope;

export type Page<T = unknown> = Omit<PageOfUnknown, 'data'> & { data: T[] };
