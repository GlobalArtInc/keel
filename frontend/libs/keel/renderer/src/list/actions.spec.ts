import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import type { Action, Descriptor } from '@globalart/keel/meta';
import { screenParamsOf } from './actions';

const FIXTURES = resolve(__dirname, '../../../../../../contracts/fixtures');
const LIST = JSON.parse(
  readFileSync(join(FIXTURES, 'descriptor-dns-zone-list.json'), 'utf8'),
) as Descriptor;

const EDIT: Action = { id: 'edit', role: 'edit', screen: 'dns.zone.edit' };
const RECORDS: Action = {
  id: 'records',
  role: 'child',
  screen: 'dns.record.list',
  params: { zone_id: '$screen.params.zone_id', view: 'all' },
};

describe('the params of a screen action', () => {
  it('a row action addresses the row by the key unless the action names params', () => {
    expect(screenParamsOf(LIST, EDIT, {}, { id: 'z2' })).toEqual({ id: 'z2' });
    expect(screenParamsOf(LIST, EDIT, {})).toEqual({});
  });

  it('a $screen.params token takes the value of the current screen', () => {
    expect(screenParamsOf(LIST, RECORDS, { zone_id: 'z1' })).toEqual({
      zone_id: 'z1',
      view: 'all',
    });
  });

  it('a $screen.params token whose value the screen lacks is dropped, the literal never passes', () => {
    const params = screenParamsOf(LIST, RECORDS, {});
    expect(params).toEqual({ view: 'all' });
    expect(Object.values(params)).not.toContain('$screen.params.zone_id');
  });

  it('a $screen.params token works together with $id in the same row action', () => {
    const action: Action = {
      id: 'records',
      role: 'child',
      screen: 'dns.record.list',
      params: { zone_id: '$screen.params.zone_id', record_id: '$id' },
    };
    expect(screenParamsOf(LIST, action, { zone_id: 'z1' }, { id: 'r7' })).toEqual({
      zone_id: 'z1',
      record_id: 'r7',
    });
  });

  it('a $screen.params token whose value is null is dropped like a missing one', () => {
    const action: Action = {
      id: 'records',
      role: 'child',
      params: { zone_id: '$screen.params.zone_id', view: 'all' },
    };
    expect(screenParamsOf(LIST, action, { zone_id: null })).toEqual({ view: 'all' });
  });

  it('a row token takes the field of the row, and a row without that field drops the key', () => {
    const renew: Action = {
      id: 'renew',
      role: 'button',
      params: { siteId: '$siteId', id: '$id' },
    };
    expect(screenParamsOf(LIST, renew, {}, { id: 'r1', siteId: 's7' })).toEqual({
      siteId: 's7',
      id: 'r1',
    });
    expect(screenParamsOf(LIST, renew, {}, { id: 'r1' })).toEqual({ id: 'r1' });
  });
});
