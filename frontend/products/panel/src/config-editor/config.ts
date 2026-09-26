import { KeelCallError, type KeelClient } from '@globalart/keel/client';
import type { CallError, Envelope, FieldError, Id } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';

export const CONFIG_EDITOR = 'hosting.site.config-editor';
export const CONFIG_GET = 'hosting.site.config.get';
export const CONFIG_SET = 'hosting.site.config.set';
export const SITE_LIST = 'hosting.site.list';
const SITE_PAGE = 100;
export const PARTS = ['server', 'pool'] as const;
export const KEYS = {
  title: `${CONFIG_EDITOR}.title`,
  site: `${CONFIG_EDITOR}.site`,
  none: `${CONFIG_EDITOR}.none`,
  pick: `${CONFIG_EDITOR}.pick`,
  server: `${CONFIG_EDITOR}.server`,
  pool: `${CONFIG_EDITOR}.pool`,
  overrides: `${CONFIG_EDITOR}.overrides`,
  rendered: `${CONFIG_EDITOR}.rendered`,
  serverHint: `${CONFIG_EDITOR}.serverHint`,
  poolHint: `${CONFIG_EDITOR}.poolHint`,
  notRendered: `${CONFIG_EDITOR}.notRendered`,
  noPool: `${CONFIG_EDITOR}.noPool`,
  save: `${CONFIG_EDITOR}.save`,
  saved: `${CONFIG_EDITOR}.saved`,
} as const;

export type ConfigPart = (typeof PARTS)[number];

export type PartErrors = Record<ConfigPart, FieldError | null>;

export interface SiteChoice {
  value: Id;
  label: string;
}

export interface SiteConfig {
  id: Id;
  domain: string;
  php: string | null;
  server: string;
  pool: string;
  rendered: Record<ConfigPart, string | null>;
}

interface SiteRow {
  id: Id;
  domain: string;
}

interface SitePage {
  data: SiteRow[];
}

export const NO_ERRORS: PartErrors = { server: null, pool: null };

export function partErrorsOf(error: CallError): PartErrors | null {
  const fields = error.fields ?? [];
  if (fields.length === 0) {
    return null;
  }
  const errors: PartErrors = { ...NO_ERRORS };
  for (const field of fields) {
    const part = PARTS.find((name) => name === field.path);
    if (part === undefined) {
      return null;
    }
    errors[part] ??= field;
  }
  return errors;
}

function settled<T>(envelope: Envelope<T>): T {
  if (!envelope.ok) {
    throw new KeelCallError(envelope.error);
  }
  return envelope.data;
}

export class ConfigApi {
  constructor(private readonly client: KeelClient) {}

  async sites(): Promise<SiteChoice[]> {
    const page = settled(
      await firstValueFrom(this.client.call<SitePage>(SITE_LIST, { page: 1, pageSize: SITE_PAGE })),
    );
    return page.data.map((row) => ({ value: row.id, label: row.domain }));
  }

  async get(id: Id): Promise<SiteConfig> {
    return settled(await firstValueFrom(this.client.call<SiteConfig>(CONFIG_GET, { id })));
  }

  async set(id: Id, config: Record<ConfigPart, string>): Promise<void> {
    settled(await firstValueFrom(this.client.call(CONFIG_SET, { id, ...config })));
  }
}
