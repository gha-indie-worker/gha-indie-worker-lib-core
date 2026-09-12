// The JSON Schema subset validator, in ESM.
//
// This is the third implementation of the same subset (Rust:
// `src/validation/json_schema.rs`; Node, dependency-free and ajv:
// `gha-indie-worker-interfaces/scripts/validate-fixtures.mjs`). They exist on
// purpose: a browser or Node client must reach the same verdict as the server
// before it sends a request, and the only way to know they agree is to run all
// of them over one fixture corpus.
//
// Zero dependencies. `node --test` is the whole test story.
//
// Supported keywords: $ref (#/$defs/*), type, required, properties,
// additionalProperties (false or a schema), enum, const, items, minItems,
// maxItems, minLength, maxLength, minimum, maximum, pattern, oneOf, and
// format-lite over uuid / date-time / date / byte. Every other keyword is a
// violation: the validator fails closed rather than ignoring a constraint.

/** @typedef {{ pointer: string, rule: string, message: string }} Violation */

const FORMATS = {
  uuid: /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/,
  'date-time': /^\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}:\d{2}(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$/,
  date: /^\d{4}-\d{2}-\d{2}$/,
  byte: /^[A-Za-z0-9+/]*={0,2}$/,
};

const ANNOTATIONS = new Set([
  'title', 'description', '$comment', 'default', 'examples', 'deprecated', '$schema', '$id', '$defs',
]);

const KEYWORDS = new Set([
  '$ref', 'type', 'required', 'properties', 'additionalProperties', 'enum', 'const',
  'items', 'minItems', 'maxItems', 'minLength', 'maxLength', 'minimum', 'maximum',
  'pattern', 'oneOf', 'format',
]);

const kindOf = (value) => {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  if (typeof value === 'number') return Number.isInteger(value) ? 'integer' : 'number';
  return typeof value;
};

const typeMatches = (expected, actual) => expected === actual || (expected === 'number' && actual === 'integer');

const escapePointer = (token) => token.replaceAll('~', '~0').replaceAll('/', '~1');

function deref(schema, root, pointer, out) {
  let current = schema;
  for (let hops = 0; current && typeof current.$ref === 'string'; hops += 1) {
    if (hops > 16) {
      out.push({ pointer, rule: 'depth', message: '$ref chain is too deep' });
      return null;
    }
    const match = /^#\/\$defs\/([A-Za-z0-9_-]+)$/.exec(current.$ref);
    if (!match) {
      out.push({ pointer, rule: 'unsupported-ref', message: `only #/$defs/<Name> is supported, got ${current.$ref}` });
      return null;
    }
    const target = (root.$defs ?? {})[match[1]];
    if (!target) {
      out.push({ pointer, rule: 'unknown-definition', message: `$ref points at missing $defs/${match[1]}` });
      return null;
    }
    current = target;
  }
  return current;
}

function walk(schema, instance, root, pointer, out) {
  if (schema === true) return;
  if (schema === false) {
    out.push({ pointer, rule: 'false-schema', message: 'nothing validates here' });
    return;
  }
  if (typeof schema !== 'object' || schema === null) {
    out.push({ pointer, rule: 'malformed-schema', message: 'schema must be an object or a boolean' });
    return;
  }
  const node = deref(schema, root, pointer, out);
  if (!node) return;

  for (const key of Object.keys(node)) {
    if (!KEYWORDS.has(key) && !ANNOTATIONS.has(key) && !key.startsWith('x-ores-')) {
      out.push({ pointer, rule: 'unsupported-keyword', message: `${key} is outside the supported subset` });
    }
  }

  const actual = kindOf(instance);
  if (node.type !== undefined) {
    const expected = Array.isArray(node.type) ? node.type : [node.type];
    if (!expected.some((t) => typeMatches(t, actual))) {
      out.push({ pointer, rule: 'type', message: `expected ${expected.join(' or ')}, got ${actual}` });
      return;
    }
  }
  if (node.const !== undefined && JSON.stringify(instance) !== JSON.stringify(node.const)) {
    out.push({ pointer, rule: 'const', message: `expected ${JSON.stringify(node.const)}` });
  }
  if (Array.isArray(node.enum) && !node.enum.some((v) => JSON.stringify(v) === JSON.stringify(instance))) {
    out.push({ pointer, rule: 'enum', message: `${JSON.stringify(instance)} is not one of ${JSON.stringify(node.enum)}` });
  }

  if (actual === 'string') {
    const length = [...instance].length;
    if (node.minLength !== undefined && length < node.minLength) {
      out.push({ pointer, rule: 'min-length', message: `shorter than ${node.minLength}` });
    }
    if (node.maxLength !== undefined && length > node.maxLength) {
      out.push({ pointer, rule: 'max-length', message: `longer than ${node.maxLength}` });
    }
    if (node.pattern !== undefined && !new RegExp(node.pattern, 'u').test(instance)) {
      out.push({ pointer, rule: 'pattern', message: `does not match ${node.pattern}` });
    }
    if (node.format !== undefined) {
      const re = FORMATS[node.format];
      if (!re) out.push({ pointer, rule: 'unsupported-format', message: `${node.format} is outside the supported subset` });
      else if (!re.test(instance)) out.push({ pointer, rule: 'format', message: `not a valid ${node.format}` });
    }
  }

  if (actual === 'number' || actual === 'integer') {
    if (node.minimum !== undefined && instance < node.minimum) {
      out.push({ pointer, rule: 'minimum', message: `below ${node.minimum}` });
    }
    if (node.maximum !== undefined && instance > node.maximum) {
      out.push({ pointer, rule: 'maximum', message: `above ${node.maximum}` });
    }
  }

  if (actual === 'array') {
    if (node.minItems !== undefined && instance.length < node.minItems) {
      out.push({ pointer, rule: 'min-items', message: `fewer than ${node.minItems} items` });
    }
    if (node.maxItems !== undefined && instance.length > node.maxItems) {
      out.push({ pointer, rule: 'max-items', message: `more than ${node.maxItems} items` });
    }
    if (node.items !== undefined) {
      instance.forEach((item, i) => walk(node.items, item, root, `${pointer}/${i}`, out));
    }
  }

  if (actual === 'object') {
    for (const name of node.required ?? []) {
      if (!Object.hasOwn(instance, name)) {
        out.push({ pointer: `${pointer}/${escapePointer(name)}`, rule: 'required', message: `${name} is required` });
      }
    }
    const properties = node.properties ?? {};
    for (const [name, child] of Object.entries(instance)) {
      const childPointer = `${pointer}/${escapePointer(name)}`;
      if (Object.hasOwn(properties, name)) {
        walk(properties[name], child, root, childPointer, out);
      } else if (node.additionalProperties === false) {
        out.push({ pointer: childPointer, rule: 'additional-properties', message: `${name} is not an allowed property` });
      } else if (node.additionalProperties && typeof node.additionalProperties === 'object') {
        walk(node.additionalProperties, child, root, childPointer, out);
      }
    }
  }

  if (Array.isArray(node.oneOf)) {
    const matched = node.oneOf.filter((branch) => {
      const scratch = [];
      walk(branch, instance, root, pointer, scratch);
      return scratch.length === 0;
    });
    if (matched.length !== 1) {
      out.push({
        pointer,
        rule: 'one-of',
        message: `expected exactly one of ${node.oneOf.length} branches to match, ${matched.length} did`,
      });
    }
  }
}

/**
 * Validate `instance` against `schema`, resolving `#/$defs/*` inside it.
 * @returns {Violation[]} empty when valid
 */
export function validate(schema, instance) {
  const out = [];
  walk(schema, instance, schema, '', out);
  return out;
}

/**
 * Validate `instance` against `#/$defs/<name>` of `document`.
 * @returns {Violation[]} empty when valid
 */
export function validateDef(document, name, instance) {
  const target = (document.$defs ?? {})[name];
  if (!target) return [{ pointer: '', rule: 'unknown-definition', message: `schema has no $defs/${name}` }];
  const out = [];
  walk(target, instance, document, name, out);
  return out;
}

/** Render violations the way the errors contract carries them. */
export function asProblemViolations(violations) {
  return violations.map(({ pointer, rule, message }) => ({ pointer: pointer || '/', rule, message }));
}
