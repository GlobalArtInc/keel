import { computed, signal, type Signal } from '@angular/core';
import { FormControl, FormGroup } from '@angular/forms';
import {
  entityPrefix,
  type Context,
  type ContextPrincipal,
  type Descriptor,
  type Expr,
  type FieldUi,
  type KeelAttrs,
  type ScreenPage,
  type UiAttrs,
} from '@globalart/keel/meta';
import { Subscription } from 'rxjs';
import { applyConditions, contextOf, holds, type ConditionScope } from './conditions';
import { mapControl, type ControlMapping, type FieldSchema } from './mapping';
import {
  defsOf,
  isObject,
  propertiesOf,
  requiredOf,
  resolveSchema,
  type JsonObject,
} from './schema';
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
}

export interface FormField {
  readonly name: string;
  readonly labelKey: string;
  readonly hintKey: string | null;
  readonly placeholderKey: string | null;
  readonly schema: JsonObject;
  readonly nullable: boolean;
  readonly keel: KeelAttrs;
  readonly ui: UiAttrs;
  readonly conditions: FieldUi;
  readonly mapping: ControlMapping;
  readonly control: FormControl<unknown>;
  readonly visible: Signal<boolean>;
  readonly required: Signal<boolean>;
  readonly readonly: Signal<boolean>;
}

export interface FormPage {
  readonly id: string;
  readonly labelKey: string;
  readonly showIf: Expr | null;
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
  field(name: string): FormField | undefined;
  page(id: string): FormPage | undefined;
  destroy(): void;
}

export function buildForm(descriptor: Descriptor, options: FormOptions = {}): KeelForm {
  const params = isObject(descriptor.params) ? descriptor.params : {};
  const properties = propertiesOf(params);
  const defs = defsOf(params);
  const requiredNames = requiredOf(params);
  const key = descriptor.key ?? null;
  const screenParams = options.params ?? {};
  const initial = options.values ?? {};
  const mode = options.mode ?? modeOf(key, screenParams, initial);
  const scope: ConditionScope = {
    mode,
    principal: options.principal ?? null,
    params: screenParams,
  };
  const values = signal<Record<string, unknown>>({});
  const site: Site = {
    prefix: entityPrefix(descriptor.name),
    properties,
    defs,
    requiredNames,
    conditions: descriptor.ui?.fields ?? {},
    context: computed(() => contextOf(values(), scope)),
    initial,
  };
  const pages: FormPage[] = [];
  const seen = new Set<string>();
  for (const declared of pagesOf(descriptor, properties)) {
    const showIf = declared.show_if ?? null;
    const visible = computed(() => holds(showIf, site.context(), true));
    const fields: FormField[] = [];
    for (const name of declared.fields ?? []) {
      if (name === key || seen.has(name) || !Object.hasOwn(properties, name)) {
        continue;
      }
      seen.add(name);
      fields.push(buildField(name, visible, site));
    }
    pages.push({
      id: declared.id,
      labelKey: `${site.prefix}.page.${declared.id}`,
      showIf,
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
  const visiblePages = computed(() => pages.filter((page) => page.visible()));
  const teardown = new Subscription();
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
    field: (name) => fields.find((field) => field.name === name),
    page: (id) => pages.find((page) => page.id === id),
    destroy: () => teardown.unsubscribe(),
  };
  applyConditions(form);
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
  initial: Record<string, unknown>;
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
    validators: fieldValidator({ name, schema: shape.schema, defs: site.defs, required }),
  });
  return {
    name,
    labelKey: `${site.prefix}.${name}`,
    hintKey: shape.ui.help ?? null,
    placeholderKey: shape.ui.placeholder ?? null,
    schema: shape.schema,
    nullable: shape.nullable,
    keel: shape.keel,
    ui: shape.ui,
    conditions: own,
    mapping: mapControl(shape),
    control,
    visible: computed(() => pageVisible() && holds(own.show_if, site.context(), true)),
    required,
    readonly: computed(() => holds(own.readonly_if, site.context(), false)),
  };
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
