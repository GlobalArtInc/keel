import { Injectable, inject } from '@angular/core';
import type { Envelope, Id, MetaPrincipal } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';
import { KeelBoot } from './boot';
import { KeelClient } from './client';
import { KeelDescriptors } from './descriptors';

export const AUTH_LOGIN = 'auth.login';
export const AUTH_TOTP_VERIFY = 'auth.totp.verify';
export const AUTH_KEY_REDEEM = 'auth.key.redeem';
export const AUTH_LOGOUT = 'auth.logout';
export const AUTH_SU = 'auth.su';
export const AUTH_SU_EXIT = 'auth.su.exit';
export const KEEL_SETUP_ADMIN = 'keel.setup.admin';

export interface LoginParams {
  login: string;
  password: string;
  method?: string;
}

export interface VerifyTotpParams {
  challenge: string;
  code: string;
}

export interface RedeemKeyParams {
  key: string;
}

export interface SuParams {
  principal: Id;
}

export interface SetupAdminParams {
  key: string;
  login: string;
  password: string;
}

export interface SetupCreated {
  id: Id;
  login: string;
}

export type LoggedIn =
  { state: 'authenticated'; principal: MetaPrincipal } | { state: 'totp'; challenge: string };

export interface Redeemed {
  redirect: string;
  principal: MetaPrincipal;
  issuedBy: Id;
}

export interface LoggedOut {
  revoked: boolean;
}

export interface Delegation {
  principal: MetaPrincipal;
}

@Injectable({ providedIn: 'root' })
export class KeelAuth {
  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly descriptors = inject(KeelDescriptors);

  login(params: LoginParams): Promise<Envelope<LoggedIn>> {
    return this.run<LoggedIn>(AUTH_LOGIN, params);
  }

  verifyTotp(params: VerifyTotpParams): Promise<Envelope<LoggedIn>> {
    return this.run<LoggedIn>(AUTH_TOTP_VERIFY, params);
  }

  redeemKey(params: RedeemKeyParams): Promise<Envelope<Redeemed>> {
    return this.run<Redeemed>(AUTH_KEY_REDEEM, params);
  }

  logout(): Promise<Envelope<LoggedOut>> {
    return this.run<LoggedOut>(AUTH_LOGOUT, {});
  }

  su(params: SuParams): Promise<Envelope<Delegation>> {
    return this.run<Delegation>(AUTH_SU, params);
  }

  suExit(): Promise<Envelope<Delegation>> {
    return this.run<Delegation>(AUTH_SU_EXIT, {});
  }

  setupAdmin(params: SetupAdminParams): Promise<Envelope<SetupCreated>> {
    return firstValueFrom(this.client.call<SetupCreated>(KEEL_SETUP_ADMIN, params));
  }

  private async run<T>(func: string, params: unknown): Promise<Envelope<T>> {
    const envelope = await firstValueFrom(this.client.call<T>(func, params));
    if (envelope.ok) {
      this.descriptors.drop();
      await this.boot.load();
    }
    return envelope;
  }
}
