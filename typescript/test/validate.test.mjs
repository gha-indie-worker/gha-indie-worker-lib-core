import test from 'node:test';
import assert from 'node:assert/strict';

import { validate, validateDef, asProblemViolations } from '../src/validate.mjs';

const schema = {
  $defs: {
    Role: { type: 'string', enum: ['owner', 'admin'] },
    Org: {
      type: 'object',
      additionalProperties: false,
      'x-ores-table': 'orgs',
      required: ['id', 'slug', 'seatLimit'],
      properties: {
        id: { type: 'string', format: 'uuid' },
        slug: { type: 'string', maxLength: 8, pattern: '^[a-z0-9-]+$' },
        seatLimit: { type: 'integer', minimum: 1, maximum: 10 },
        role: { $ref: '#/$defs/Role' },
        tags: { type: 'array', items: { type: 'string' }, maxItems: 2 },
        createdAt: { type: 'string', format: 'date-time' },
      },
    },
  },
};

const org = () => ({ id: '00000001-1111-4222-8333-444455556666', slug: 'indie', seatLimit: 5 });
const rules = (violations) => violations.map((v) => v.rule);

test('a valid instance passes', () => {
  assert.deepEqual(validateDef(schema, 'Org', org()), []);
});

test('missing required property is reported with a pointer', () => {
  const { slug, ...rest } = org();
  const found = validateDef(schema, 'Org', rest);
  assert.equal(found.length, 1);
  assert.equal(found[0].rule, 'required');
  assert.equal(found[0].pointer, 'Org/slug');
});

test('sealed objects reject extra properties', () => {
  assert.deepEqual(rules(validateDef(schema, 'Org', { ...org(), tier: 'gold' })), ['additional-properties']);
});

test('scalar rules are enforced', () => {
  const found = rules(validateDef(schema, 'Org', { ...org(), slug: 'WAY-TOO-LONG', seatLimit: 99 }));
  assert.ok(found.includes('max-length'));
  assert.ok(found.includes('pattern'));
  assert.ok(found.includes('maximum'));
});

test('refs resolve into $defs', () => {
  assert.deepEqual(rules(validateDef(schema, 'Org', { ...org(), role: 'member' })), ['enum']);
  assert.deepEqual(validateDef(schema, 'Org', { ...org(), role: 'admin' }), []);
});

test('arrays check items and bounds', () => {
  const found = rules(validateDef(schema, 'Org', { ...org(), tags: ['a', 2, 'c'] }));
  assert.ok(found.includes('type'));
  assert.ok(found.includes('max-items'));
});

test('unsupported keywords fail closed', () => {
  assert.deepEqual(rules(validate({ type: 'string', contentEncoding: 'base64' }, 'hi')), ['unsupported-keyword']);
});

test('oneOf selects exactly one branch', () => {
  const union = {
    type: 'object',
    properties: { kind: { type: 'string' }, runId: { type: 'string' } },
    oneOf: [
      { properties: { kind: { const: 'subscribe' } }, required: ['kind', 'runId'] },
      { properties: { kind: { const: 'heartbeat' } }, required: ['kind'] },
    ],
  };
  assert.deepEqual(validate(union, { kind: 'heartbeat' }), []);
  assert.deepEqual(validate(union, { kind: 'subscribe', runId: 'r' }), []);
  assert.deepEqual(rules(validate(union, { kind: 'subscribe' })), ['one-of']);
});

test('integers and numbers are distinguished', () => {
  assert.deepEqual(validate({ type: 'integer' }, 3), []);
  assert.equal(validate({ type: 'integer' }, 3.5).length, 1);
  assert.deepEqual(validate({ type: 'number' }, 3), []);
  assert.deepEqual(validate({ type: 'number' }, 3.5), []);
});

test('formats are checked', () => {
  const stamp = { type: 'string', format: 'date-time' };
  assert.deepEqual(validate(stamp, '2026-03-14T09:26:53Z'), []);
  assert.deepEqual(validate(stamp, '2026-03-14T09:26:53.123+01:00'), []);
  assert.equal(validate(stamp, '2026-03-14 09:26:53').length, 1);
  assert.equal(validate({ type: 'string', format: 'date' }, '2026-3-14').length, 1);
});

test('additionalProperties may be a schema', () => {
  const clock = { type: 'object', additionalProperties: { type: 'integer', minimum: 0 } };
  assert.deepEqual(validate(clock, { 'api-1': 2 }), []);
  assert.equal(validate(clock, { 'api-1': -1 }).length, 1);
});

test('an unknown definition is a violation, not a throw', () => {
  assert.equal(validateDef(schema, 'Nope', org())[0].rule, 'unknown-definition');
});

test('violations render as problem violations', () => {
  const rows = asProblemViolations(validate({ type: 'object', required: ['id'] }, {}));
  assert.equal(rows.length, 1);
  assert.equal(rows[0].pointer, '/id');
  assert.equal(rows[0].rule, 'required');
});
