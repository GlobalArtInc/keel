import { inject, makeEnvironmentProviders, type EnvironmentProviders } from '@angular/core';
import { ROUTES, provideRouter, withComponentInputBinding, type Routes } from '@angular/router';
import { injectKeelWidgets, provideKeelWidgets } from '@globalart/keel/forms';
import { KeelScreenComponent, SCREEN_PATH } from '@globalart/keel/renderer';
import {
  KeelStartup,
  LOGIN_PATH,
  LOGOUT_PATH,
  keelGuard,
  keelHome,
  keelLogout,
  keelReady,
} from './guard';
import { KeelHomeComponent } from './home/home.component';
import { KeelLoginComponent } from './login/login.component';
import { KeelNotFoundComponent } from './not-found';
import { KeelProducts } from './products';
import { KeelLayoutComponent } from './layout/layout.component';
import { provideKeelShellViews } from './views';

export const FUNC_PARAM = 'func';

export function screenRoutes(providers: readonly EnvironmentProviders[]): Routes {
  return [{ path: `:${FUNC_PARAM}`, component: KeelScreenComponent, providers: [...providers] }];
}

export function keelRoutes(): Routes {
  const startup = inject(KeelStartup);
  const products = inject(KeelProducts);
  const widgets = provideKeelWidgets(injectKeelWidgets());
  const views = provideKeelShellViews();
  return [
    { path: LOGIN_PATH, component: KeelLoginComponent, canActivate: [keelReady] },
    { path: LOGOUT_PATH, canActivate: [keelLogout], children: [] },
    {
      path: '',
      component: KeelLayoutComponent,
      canActivate: [keelGuard],
      children: [
        {
          path: '',
          pathMatch: 'full',
          component: KeelHomeComponent,
          canActivate: [keelHome],
          providers: [widgets, views],
        },
        {
          path: SCREEN_PATH,
          canMatch: [keelReady],
          loadChildren: () =>
            startup
              .ready()
              .then((boot) => products.load(boot))
              .then((loaded) => screenRoutes([widgets, views, ...loaded])),
        },
        { path: '**', component: KeelNotFoundComponent },
      ],
    },
  ];
}

export function provideKeelRouter(): EnvironmentProviders {
  return makeEnvironmentProviders([
    provideRouter([], withComponentInputBinding()),
    { provide: ROUTES, multi: true, useFactory: keelRoutes },
  ]);
}
