import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import {
  KEEL_WIDGETS,
  injectKeelWidgets,
  provideKeelWidgets,
  widgetOf,
} from '@globalart/keel/forms';

@Component({ selector: 'keel-test-money', template: '' })
class MoneyWidgetComponent {}

@Component({ selector: 'keel-test-cidr', template: '' })
class CidrWidgetComponent {}

describe('provideKeelWidgets', () => {
  it('registers product widgets under a name and hands them to the form model', () => {
    TestBed.configureTestingModule({
      providers: [
        provideKeelWidgets([{ name: 'money', component: MoneyWidgetComponent }]),
        provideKeelWidgets([{ name: 'cidr', component: CidrWidgetComponent }]),
      ],
    });
    const widgets = TestBed.runInInjectionContext(injectKeelWidgets);
    expect(widgets.map((widget) => widget.name)).toEqual(['money', 'cidr']);
    expect(widgetOf(widgets, 'cidr')?.component).toBe(CidrWidgetComponent);
    expect(widgetOf(widgets, 'cpu')).toBeUndefined();
    expect(TestBed.inject(KEEL_WIDGETS)).toHaveLength(2);
  });

  it('answers no widgets when none are provided', () => {
    TestBed.configureTestingModule({});
    expect(TestBed.runInInjectionContext(injectKeelWidgets)).toEqual([]);
  });

  it('refuses a name registered twice in one call', () => {
    expect(() =>
      provideKeelWidgets([
        { name: 'money', component: MoneyWidgetComponent },
        { name: 'money', component: CidrWidgetComponent },
      ]),
    ).toThrow('the widget money is registered twice');
  });
});
