import { TestBed } from '@angular/core/testing';
import { KEEL_SHELL_VERSION } from '@globalart/keel/shell';
import { AppComponent } from './app.component';

describe('AppComponent', () => {
  it('renders the platform name and the shell version on the fullscreen layout', async () => {
    await TestBed.configureTestingModule({ imports: [AppComponent] }).compileComponents();
    const fixture = TestBed.createComponent(AppComponent);
    fixture.detectChanges();
    const element: HTMLElement = fixture.nativeElement;
    expect(element.querySelector('phx-fullscreen-layout')).not.toBeNull();
    expect(element.querySelector('h1')?.textContent).toBe('Keel');
    expect(element.querySelector('p')?.textContent).toBe(KEEL_SHELL_VERSION);
  });
});
