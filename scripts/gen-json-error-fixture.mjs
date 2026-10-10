// Generates tests/fixtures/node-json-errors.json (VE-3728): V8's `JSON.parse` error message
// for malformed text, which the TS CLI printed for a non-JSON 2xx response body and inside
// "Failed to read <file>: …" for a bad --*-file. `null` means the text parses.
//
//   node scripts/gen-json-error-fixture.mjs > tests/fixtures/node-json-errors.json
const long = 'x'.repeat(30);
const inputs = [
  // Empty, whitespace and HTML/text bodies.
  '', ' ', '\n', '\t\r\n ', '<', '<html>', '<!DOCTYPE html><html><body>Bad gateway</body></html>',
  'OK', 'ok', 'x', 'abcdefghij', 'abcdefghijk', '01234567890123456789', 'Internal Server Error',
  '<?xml version="1.0"?><error>Gateway Timeout</error>', 'Bad Gateway: upstream connect error or disconnect',
  // Objects and arrays cut short or malformed.
  '{', '{"a"', '{"a":', '{"a":1', '{"a":1,', '{"a":1,}', '[', '[1', '[1,', '[1,]', '[1 2]', '{"a" 1}', '{a:1}',
  "{'a':1}", '{"a":1}x', '{"a":1} {}', '{"a":1}}', ']', '}', ',', ':', '{,}', '{"a",}', '[,1]', '[["a" "b"]]',
  '{"a":1 "b":2}', '{"a":1,"b"}', '{"data": [1, 2, 3}', '{"a":{"b":[{"c":}]}}', '[[[[[[[[[[[[]]]]]]]]]]',
  // Strings and escapes.
  '"abc', '"unterminated string that is long', '"a\\x"', '"a\\u12"', '"\\u00zz"', '"\\uZZZZ"', '"\\u12', '"\\',
  '"a\nb"', '"\t"', '"\u0001"', '"\\/"', '{"\\u0041":1,', '{"a":"\\ud800"}', '["\\ud83d\\ude00"]', '{"k":"v"x}',
  // Numbers.
  '-', '-a', '1.', '1.e5', '1e', '1e+', '01', '00', '+1', '.5', '[-]', '[1e]', '{"a":01}', '123abc', '-0x1',
  '{"a":-}', '1234567890123', '-0', '0.0e-0', '1E+2', '1234567890.5', '1234567890e2', '12345678901.', '-1234567890123.5e',
  '0e', '0.', '-0.', '-00', '0-1', '[1.5e+]', '[12345678901234567890]',
  // Literals.
  'tru', 'true x', 'nul', 'nulls', 'fals', 'falsey', 'tr1', 'tr"x"', 't"', 'nu,', '{"a":tru}', '[nul]',
  'NaN', 'Infinity', 'undefined', '[object Object]', '-Infinity', 'null', 'true',
  // Non-ASCII text, BOM, NUL, line endings and context windows.
  'é', 'é{', '{"x":"é"} é', '["😀", x]', '﻿{}', '{"a":1}\u0000', '[\n1,\n2\n,]',
  '{\n  "a": 1,\n  "b": [\n    1,\n  ]\n}', '{\r\n"a":1,\r\n}', '{\r"a":1,\r}', '[1,\r\n\r\n2,\n\rx]',
  `{"key": "value", "other": undefined}`, `{"a":"${long}", "b": nope, "c": "${long}"}`, `[${long}]`,
  `{"${long}":1,}`, `["éééééééééééé", x, "ééééééééééé"]`,
  '{"emoji":"😀😀😀", x}',
];

const cases = inputs.map((text) => {
  try {
    JSON.parse(text);
    return [text, null];
  } catch (err) {
    return [text, err.message];
  }
});
console.log(JSON.stringify({ node: process.version, v8: process.versions.v8, cases }, null, 1));
