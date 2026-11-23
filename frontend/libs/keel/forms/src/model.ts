import { computed, signal, untracked, type Signal, type WritableSignal } from '@angular/core';
import { FormControl, FormGroup } from '@angular/forms';
import type { KeelClient } from '@globalart/keel/client';
import {
  entityPrefix,
  type Choice,
  type Context,
  type ContextPrincipal,
  type Descriptor,
  type Envelope,
  type Expr,
  type FieldError,
  type FieldUi,
  type KeelAttrs,
  type ScreenPage,
  type UiAttrs,
} from '@globalart/keel/meta';
import { Subscription } from 'rxjs';
import { applyConditions, contextOf, holds, type ConditionScope } from './conditions';
import { mapControl, type ControlMapping, type FieldSchema } from './mapping';
import { RemoteOptions } from './options';
import { bindRecompute } from './recompute';
import {
  defsOf,
  isObject,
  propertiesOf,
  requiredOf,
  resolveSchema,
  type JsonObject,
} from './schema';
import { newIdempotencyKey, submitForm } from './submit';
import { fieldValidator, isEmptyValue } from './validators';
import type { KeelWidget } from './widgets';

export type FormMode = 'create' | 'edit';

export type FormLayout = 'section' | 'tabs';

export const MAIN_PAGE = 'main';

export interface FormOptions {
  mode?: FormMode;
  params?: Record<string, unknown>;
  principal?: ContextPrincipal | null;
  values?: Record<string, unknown>;
  widgets?: readonly KeelWidget[];
  client?: KeelClient;
}

export interface FormField {
  readonly name: string;
  readonly labelKey: string;
  readonly hintKey: string | null;
  readonly checkHintKey: Signal<string | null>;
  readonly placeholderKey: string | null;
  readonly schema: JsonObject;
  readonly nullable: boolean;
  readonly keel: KeelAttrs;
  readonly ui: UiAttrs;
  readonly conditions: FieldUi;
  readonly mapping: ControlMapping;
  readonly control: FormControl<unknown>;
  readonly options: RemoteOptions | null;
  readonly choices: WritableSignal<readonly Choice[] | null>;
  readonly visible: Signal<boolean>;
  readonly required: Signal<boolean>;
  readonly readonly: Signal<boolean>;
}

export interface FormPage {
  readonly id: string;
  readonly labelKey: string;
  readonly showIf: Expr | null;
  readonly validate: string | null;
  readonly fields: readonly FormField[];
  readonly visible: Signal<boolean>;
}

export type FormControls = Record<string, FormControl<unknown>>;

export interface KeelForm {
  readonly descriptor: Descriptor;
  readonly name: string;
  readonly title: string;
  readonly mode: FormMode;
  readonly key: string | null;
  readonly group: FormGroup<FormControls>;
  readonly pages: readonly FormPage[];
  readonly fields: readonly FormField[];
  readonly visiblePages: Signal<readonly FormPage[]>;
  readonly layout: Signal<FormLayout>;
  readonly values: Signal<Record<string, unknown>>;
  readonly context: Signal<Context>;
  readonly widgets: readonly KeelWidget[];
  readonly client: KeelClient | null;
  readonly unmapped: WritableSignal<readonly FieldError[]>;
  readonly pending: Signal<boolean>;
  field(name: string): FormField | undefined;
  page(id: string): FormPage | undefined;
  submit<T = unknown>(): Promise<Envelope<T> | null>;
  destroy(): void;
}

export function buildForm(descriptor: Descriptor, options: FormOptions = {}): KeelForm {
  const params = isObject(descriptor.params) ? descriptor.params : {};
  const properties = propertiesOf(params);
  const defs = defsOf(params);
  const requiredNames = requiredOf(params);
  const key = descriptor.key ?? null;
  const screenParams = options.params ?? {};
  const given = options.values ?? {};
  const mode = options.mode ?? modeOf(key, screenParams, given);
  const initial = mode === 'create' ? { ...carriedOf(screenParams, given), ...given } : given;
  const scope: ConditionScope = {
    mode,
    principal: options.principal ?? null,
    params: screenParams,
  };
  const values = signal<Record<string, unknown>>({});
  const client = options.client ?? null;
  const site: Site = {
    prefix: entityPrefix(descriptor.name),
    properties,
    defs,
    requiredNames,
    conditions: descriptor.ui?.fields ?? {},
    context: computed(() => contextOf(values(), scope)),
    values,
    initial,
    client,
    scope: screenParams,
  };
  const pages: FormPage[] = [];
  const seen = new Set<string>();
  for (const declared of pagesOf(descriptor, properties)) {
    const showIf = declared.show_if ?? null;
    const visible = computed(() => holds(showIf, site.context(), true));
    const fields: FormField[] = [];
    for (const name of declared.fields ?? []) {
      if (seen.has(name) || !Object.hasOwn(properties, name)) {
        continue;
      }
      seen.add(name);
      const field = buildField(name, visible, site);
      if (name === key && isEmptyValue(field.control.value) && !isEmptyValue(screenParams[name])) {
        field.control.setValue(screenParams[name], { emitEvent: false });
      }
      fields.push(field);
    }
    pages.push({
      id: declared.id,
      labelKey: `${site.prefix}.page.${declared.id}`,
      showIf,
      validate: declared.validate ?? null,
      fields: fields.sort(byOrder),
      visible,
    });
  }
  const fields = pages.flatMap((page) => page.fields);
  const controls: FormControls = {};
  for (const name of Object.keys(properties)) {
    const field = fields.find((entry) => entry.name === name);
    if (field !== undefined) {
      controls[name] = field.control;
      continue;
    }
    const seed = name === key ? { [name]: initial[name] ?? screenParams[name] } : initial;
    controls[name] = new FormControl<unknown>(initialOf(shapeOf(name, site), seed));
  }

  const group = new FormGroup<FormControls>(controls);
  values.set(group.getRawValue());
  revalidateChoices(fields);
  const visiblePages = computed(() => pages.filter((page) => page.visible()));
  const teardown = new Subscription();
  const pending = signal(false);
  let idempotencyKey: string | null = null;
  const form: KeelForm = {
    descriptor,
    name: descriptor.name,
    title: descriptor.ui?.title ?? descriptor.name,
    mode,
    key,
    group,
    pages,
    fields,
    visiblePages,
    layout: computed(() => (visiblePages().length > 1 ? 'tabs' : 'section')),
    values: values.asReadonly(),
    context: site.context,
    widgets: options.widgets ?? [],
    client,
    unmapped: signal<readonly FieldError[]>([]),
    pending: pending.asReadonly(),
    field: (name) => fields.find((field) => field.name === name),
    page: (id) => pages.find((page) => page.id === id),
    submit: async <T>() => {
      if (client === null) {
        throw new TypeError(`the form ${descriptor.name} was built without a client`);
      }
      idempotencyKey ??= newIdempotencyKey();
      pending.set(true);
      try {
        const envelope = await submitForm<T>(form, client, idempotencyKey);
        if (envelope?.ok) {
          idempotencyKey = null;
        }
        return envelope;
      } finally {
        pending.set(false);
      }
    },
    destroy: () => {
      teardown.unsubscribe();
      for (const field of fields) {
        field.options?.destroy();
      }
    },
  };
  applyConditions(form);
  if (client !== null) {
    teardown.add(bindRecompute(form, client));
  }
  let applying = false;
  teardown.add(
    group.valueChanges.subscribe(() => {
      if (applying) {
        return;
      }
      applying = true;
      try {
        values.set(group.getRawValue());
        applyConditions(form);
        revalidateChoices(form.fields);
      } finally {
        applying = false;
      }
    }),
  );
  return form;
}

interface Site {
  prefix: string;
  properties: JsonObject;
  defs: JsonObject;
  requiredNames: string[];
  conditions: Record<string, FieldUi>;
  context: Signal<Context>;
  values: Signal<Record<string, unknown>>;
  initial: Record<string, unknown>;
  client: KeelClient | null;
  scope: Record<string, unknown>;
}

function buildField(name: string, pageVisible: Signal<boolean>, site: Site): FormField {
  const shape = shapeOf(name, site);
  const own: FieldUi = site.conditions[name] ?? {};
  const required = computed(() =>
    own.required_if
      ? holds(own.required_if, site.context(), false)
      : site.requiredNames.includes(name),
  );
  const control = new FormControl<unknown>(initialOf(shape, site.initial), {
    validators: fieldValidator({
      name,
      schema: shape.schema,
      defs: site.defs,
      required,
      siblings: () => untracked(site.values),
    }),
  });
  const labelKey = `${site.prefix}.${name}`;
  return {
    name,
    labelKey,
    hintKey: shape.ui.help ?? null,
    checkHintKey: computed(() => checkHintKeyOf(labelKey, shape.keel, site)),
    placeholderKey: shape.ui.placeholder ?? null,
    schema: shape.schema,
    nullable: shape.nullable,
    keel: shape.keel,
    ui: shape.ui,
    conditions: own,
    mapping: mapControl(shape),
    control,
    options: remoteOptionsOf(shape, site),
    choices: signal<readonly Choice[] | null>(null),
    visible: computed(() => pageVisible() && holds(own.show_if, site.context(), true)),
    required,
    readonly: computed(() => holds(own.readonly_if, site.context(), false)),
  };
}

function checkHintKeyOf(labelKey: string, keel: KeelAttrs, site: Site): string | null {
  const source = keel.checkBy?.field;
  const value = source === undefined ? undefined : site.values()[source];
  return typeof value === 'string' ? `${labelKey}.hint.${value}` : null;
}

function revalidateChoices(fields: readonly FormField[]): void {
  for (const field of fields) {
    if (field.keel.checkBy !== undefined) {
      field.control.updateValueAndValidity({ emitEvent: false });
    }
  }
}

function remoteOptionsOf(shape: FieldSchema, site: Site): RemoteOptions | null {
  const func = shape.keel.options ?? null;
  if (func === null || site.client === null) {
    return null;
  }
  return new RemoteOptions(site.client, func, site.scope);
}

function shapeOf(name: string, site: Site): FieldSchema {
  const resolved = resolveSchema(site.properties[name], site.defs);
  return {
    name,
    schema: resolved.schema,
    nullable: resolved.nullable,
    type: resolved.type,
    keel: attrsOf<KeelAttrs>(resolved.schema['x-keel']),
    ui: attrsOf<UiAttrs>(resolved.schema['x-ui']),
    defs: site.defs,
  };
}

function carriedOf(
  params: Record<string, unknown>,
  given: Record<string, unknown>,
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(params).filter(
      ([name, value]) => given[name] === undefined && value !== undefined,
    ),
  );
}

function modeOf(
  key: string | null,
  params: Record<string, unknown>,
  values: Record<string, unknown>,
): FormMode {
  if (key === null) {
    return 'create';
  }
  return isEmptyValue(params[key] ?? values[key]) ? 'create' : 'edit';
}

function pagesOf(descriptor: Descriptor, properties: JsonObject): ScreenPage[] {
  const pages = descriptor.ui?.pages ?? [];
  if (pages.length > 0) {
    return pages;
  }
  const fields = Object.keys(properties).filter((name) => name !== descriptor.key);
  return [{ id: MAIN_PAGE, fields }];
}

function attrsOf<T extends object>(value: unknown): T {
  return (isObject(value) ? value : {}) as T;
}

function initialOf(shape: FieldSchema, values: Record<string, unknown>): unknown {
  if (Object.hasOwn(values, shape.name) && values[shape.name] !== undefined) {
    return values[shape.name];
  }
  if (shape.schema['default'] !== undefined) {
    return shape.schema['default'];
  }
  switch (shape.type) {
    case 'boolean':
      return false;
    case 'array':
      return [];
    case 'object':
      return {};
    default:
      return null;
  }
}

function byOrder(left: FormField, right: FormField): number {
  const first = orderOf(left);
  const second = orderOf(right);
  return first === second ? 0 : first < second ? -1 : 1;
}

function orderOf(field: FormField): number {
  return field.ui.order ?? Number.POSITIVE_INFINITY;
}
