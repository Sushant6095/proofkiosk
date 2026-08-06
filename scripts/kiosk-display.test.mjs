import test from 'node:test';
import assert from 'node:assert/strict';
import { stageOf } from './kiosk-display.mjs';

const order = { reference: 'REF', expires_at_ms: 1_000 };
const claim = { claim_id: 'C1' };
const ev = (event) => ({ event, reference: 'REF' });

test('an order with nothing behind it is awaiting', () => {
  assert.equal(stageOf(order, null, [], 500), 'awaiting');
});

test('an unpaid order past its quote window is expired', () => {
  assert.equal(stageOf(order, null, [], 2_000), 'expired');
});

test('a claim outranks the expiry clock — a paid order never reverts to expired', () => {
  // The claim layer already proved the payment landed inside the window, so a
  // later wall-clock reading must not un-sell a delivered item.
  assert.equal(stageOf(order, claim, [], 2_000), 'claimed');
});

test('actuating shows as dispensing', () => {
  assert.equal(stageOf(order, claim, [ev('actuating')], 500), 'dispensing');
});

test('delivered outranks actuating', () => {
  assert.equal(stageOf(order, claim, [ev('actuating'), ev('delivered')], 500), 'delivered');
});

test('a refusal recorded after delivery does not erase the delivery', () => {
  // The actuator journals a refusal on every replay attempt. Those are the
  // guard working, not a failed delivery, and must not roll the screen back.
  const events = [ev('actuating'), ev('delivered'), ev('refused')];
  assert.equal(stageOf(order, claim, events, 500), 'delivered');
});

test('a refusal with no delivery leaves the order at its claim stage', () => {
  assert.equal(stageOf(order, claim, [ev('refused')], 500), 'claimed');
});
