export { MAIN_PAGE, buildForm } from './model';
export type {
  FormControls,
  FormField,
  FormLayout,
  FormMode,
  FormOptions,
  FormPage,
  KeelForm,
} from './model';
export { CONTROLS, mapControl } from './mapping';
export type { ControlMapping, FieldSchema, InputType, PhxControl } from './mapping';
export { applyConditions, contextOf, holds } from './conditions';
export type { ConditionScope } from './conditions';
export {
  FINDINGS,
  REQUIRED_CODE,
  REQUIRED_KEY,
  fieldValidator,
  findingsOf,
  isEmptyValue,
  validateField,
} from './validators';
export type { FieldRule } from './validators';
export { resolveSchema } from './schema';
export type { ResolvedSchema } from './schema';
export { KEEL_WIDGETS, injectKeelWidgets, provideKeelWidgets, widgetOf } from './widgets';
export type { KeelWidget } from './widgets';
export { KeelFormComponent } from './form.component';
export { KeelPageComponent } from './page.component';
export { KeelFieldComponent } from './field.component';

export const KEEL_FORMS_VERSION = '0.1.0';
